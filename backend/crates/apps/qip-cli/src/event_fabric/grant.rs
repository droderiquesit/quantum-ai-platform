//! ADR 0100 §8: `qip event-fabric grant` — the labelled fixture, signed with
//! the dev key, that pushes P0 control down to `qip-edge-node`. SLICE-34.
//!
//! The first vertical slice proves P0 control over the fabric with an
//! operator-signed fixture rather than the central plane's own issue path,
//! which is a named follow-on packet. ADR 0100's "What would make this
//! wrong" names the failure this module exists to make hard: **the fixture
//! grant path surviving and becoming how grants are issued.** Every fence
//! below is aimed at that, and each is a refusal rather than a warning.
//!
//! - **Loopback only.** `--peer` must resolve to loopback and nothing else;
//!   a name resolving to a loopback address *and* a routable one is refused,
//!   and the transport is opened to the checked address, never to the name
//!   again, so a second resolution cannot move it. A fixture that could
//!   reach a regional fabric would be a grant path.
//! - **Labelled, in three places.** The command will not run without
//!   `--fixture`; the fixture file must carry
//!   `"label": "fixture:qip-event-fabric-grant"`; and the key file must begin
//!   with [`FIXTURE_KEY_LABEL`]. The last one is the fence that matters most:
//!   a production envelope key mounted here by mistake is refused, so this
//!   command can never mint a grant a production cell would accept.
//! - **Signed as the fixture.** Every envelope names [`FIXTURE_SIGNATORY`]
//!   as its signatory — the grant's approver and each frame's lineage
//!   producer — so a journal that records one shows where it came from
//!   without anybody's memory of how it got there.
//! - **Paper only.** The grant names [`SIMULATED_VENUE`] and nothing else,
//!   and nothing here can name an autonomy level: [`PolicyPayload`] has no
//!   field that could carry one.
//!
//! The key and the release controller's bearer token are read only through
//! `qip_core::secret`'s `_FILE` indirection. The direct variable is refused
//! rather than preferred: a key in the environment is a key in
//! `/proc/<pid>/environ` and every child process. Neither value is printed,
//! logged or placed in an error; [`FixtureKey`]'s `Debug` is redacted and it
//! has no `Clone`, following `BearerToken`.
//!
//! **HMAC, not a signature.** The grant and payload are MACed with a shared
//! key through the cell's own construction (`qip_edge::envelope::sign_payload`,
//! `PolicyPayload::signed`). That proves possession of the key, not the
//! identity of a signer; ADR 0043 names asymmetric signing as a gap no
//! in-tree code may close, and this module does not try.

use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{GrantManifest, PlanDigest, PolicyPayload, Slot};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::ids::EventId;
use qip_core::time::Duration;
use qip_core::{CorrelationId, Decimal, Lineage, Timestamp};
use qip_edge::envelope::sign_payload;
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::AckProfile;
use qip_events::{AnyEvent, Envelope, EventBody};
use qip_mesh::spine::{CapitalGrantFrame, PolicyFrame};
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::protocol::ProduceAck;
use qip_transport::event_fabric::transport::{FabricTransport, HttpTransport, Timeouts};
use qip_transport::retry::RetryPolicy;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

use super::{Environment, Outcome};

/// The subcommand's name in `qip event-fabric <subcommand>`.
pub const SUBCOMMAND: &str = "grant";

/// The signatory every envelope this command signs names, and the label the
/// fixture file must carry.
///
/// One string for both on purpose: the file says what it is with the same
/// words the journal will later show it signed as.
pub const FIXTURE_SIGNATORY: &str = "fixture:qip-event-fabric-grant";

/// The only venue a fixture grant may name.
///
/// `XSIM` is the simulated venue the deep brain's own simulations already
/// name. A fixture that could name a real venue identifier would be a grant a
/// cell configured for that venue would act on.
pub const SIMULATED_VENUE: &str = "XSIM";

/// The identity a P0 control stream admits a producer under.
/// `qip_events::event_fabric::catalogue` refuses a produce grant on a P0
/// stream to anybody else, so this is the only name the command publishes as.
pub const RELEASE_CONTROLLER: &str = "release-controller";

/// The variable whose `_FILE` form names the fixture key. The direct form is
/// refused.
pub const KEY_VARIABLE: &str = "QIP_EVENT_FABRIC_FIXTURE_KEY";

/// The variable whose `_FILE` form names the release controller's bearer
/// token. The direct form is refused.
pub const TOKEN_VARIABLE: &str = "QIP_EVENT_FABRIC_RELEASE_CONTROLLER_TOKEN";

/// What the fixture key file must begin with.
///
/// Part of the key: the MAC is taken over the whole file, label included, so
/// a cell verifying a fixture grant is configured with a key that says what
/// it is wherever it is mounted. A key without it is refused here, which is
/// what keeps a production envelope key from ever signing a fixture.
pub const FIXTURE_KEY_LABEL: &str = "qip-fixture-key:";

/// The least key material after the label: 256 bits of hex, the size of the
/// MAC it keys.
pub const MINIMUM_KEY_MATERIAL: usize = 32;

/// The longest window a fixture grant may be valid for.
///
/// A day: the slice is a session on a workstation. A fixture grant that
/// outlived that would still be funding a cell after everybody had forgotten
/// it was issued, which is the fixture surviving by another route.
pub const MAXIMUM_VALIDITY_SECS: i64 = 24 * 60 * 60;

/// The largest fixture or plan file read. Both are a handful of lines; a file
/// past this is not the file that was meant.
pub const MAXIMUM_FILE_BYTES: u64 = 64 * 1024;

/// The batch header's schema id. No numeric schema registry assigns one yet;
/// each record's `AnyEvent` names its own topic and schema version, and that
/// is what the cell's downlink checks before it decodes anything.
const UNREGISTERED_SCHEMA_ID: u32 = 0;

/// The batch header's schema version, for the same reason.
const BATCH_SCHEMA_VERSION: u32 = 1;

/// Fixed so that two runs of the command retry on the same schedule: a
/// replay that diverges in its retry timing is a replay that diverges.
const RETRY_SEED: u64 = 0x5eed_0034;
const BREAKER_SEED: u64 = 0x5eed_0035;

/// The two things the command is told on its command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantArguments {
    pub fixture: PathBuf,
    pub peer: String,
}

/// Parse `--fixture <path> --peer <addr>`, in either order.
///
/// `--fixture` is required, and that is a fence rather than a convenience:
/// there is no default fixture, so the word is in every shell history that
/// ran this command and no invocation signs anything that did not name the
/// labelled file.
pub fn parse_arguments(arguments: &[String]) -> Result<GrantArguments> {
    let mut found: BTreeMap<&str, &str> = BTreeMap::new();
    let mut rest = arguments.iter();
    while let Some(argument) = rest.next() {
        let name = match argument.as_str() {
            "--fixture" => "--fixture",
            "--peer" => "--peer",
            other => {
                return Err(Error::invalid(format!(
                    "unknown argument {other:?}; usage: qip event-fabric grant --fixture <path> \
                     --peer <loopback-address:port>"
                )));
            }
        };
        let Some(value) = rest.next() else {
            return Err(Error::invalid(format!("{name} needs a value after it")));
        };
        if found.insert(name, value.as_str()).is_some() {
            return Err(Error::invalid(format!(
                "{name} was given twice; which one is meant is not something this command will \
                 guess"
            )));
        }
    }
    let Some(fixture) = found.get("--fixture") else {
        return Err(Error::invalid(format!(
            "`qip event-fabric grant` signs only a labelled fixture and will not run without \
             --fixture <path>, a file labelled {FIXTURE_SIGNATORY:?}. It is not how grants are \
             issued; the central plane's issue path is"
        )));
    };
    let Some(peer) = found.get("--peer") else {
        return Err(Error::invalid(
            "`qip event-fabric grant` needs --peer <address:port>, a fabric broker on loopback",
        ));
    };
    Ok(GrantArguments {
        fixture: PathBuf::from(fixture),
        peer: (*peer).to_string(),
    })
}

/// The one address the command will connect to, refused unless every
/// address `peer` resolves to is loopback.
///
/// Every, not any: a name that resolves to `127.0.0.1` and to a routable
/// address could be connected to either, and the one this check saw is not
/// necessarily the one a connection would pick. The caller connects to the
/// returned address and never resolves `peer` again.
pub fn loopback_peer(peer: &str) -> Result<SocketAddr> {
    let resolved: Vec<SocketAddr> = peer
        .to_socket_addrs()
        .map_err(|error| {
            Error::invalid(format!(
                "--peer {peer:?} is not an address:port this command can resolve: {error}"
            ))
        })?
        .collect();
    if let Some(outside) = resolved.iter().find(|address| !address.ip().is_loopback()) {
        return Err(Error::denied(format!(
            "--peer {peer:?} resolves to {outside}, which is not loopback. The fixture grant \
             reaches a broker on this machine only; a fixture that could reach a regional fabric \
             would be a grant path, and that is the central plane's"
        )));
    }
    resolved.first().copied().ok_or_else(|| {
        Error::invalid(format!(
            "--peer {peer:?} resolved to no address at all, so there is nothing to check as \
             loopback"
        ))
    })
}

/// The slice's control grant, as the fixture file states it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixture {
    pub cell: String,
    pub region: String,
    pub strategy: String,
    pub venue: String,
    pub gross_limit: Decimal,
    pub order_limit: Decimal,
    pub loss_limit: Decimal,
    pub valid_for_secs: i64,
    pub policy_sequence: u64,
    pub partition: u32,
    /// The digest of the plan file the fixture names, computed here from its
    /// bytes rather than copied into the fixture, so the payload cannot name
    /// a plan other than the one on disk.
    pub plan: PlanDigest,
}

impl Fixture {
    /// The P0 stream this fixture's control goes to.
    pub fn stream(&self) -> String {
        format!("control.{}", self.region)
    }
}

/// The fields a fixture file must carry, and nothing else may appear.
const FIXTURE_FIELDS: [&str; 12] = [
    "label",
    "cell",
    "region",
    "strategy",
    "venue",
    "gross_limit",
    "order_limit",
    "loss_limit",
    "valid_for_secs",
    "policy_sequence",
    "partition",
    "plan",
];

/// Read and check a fixture file, and digest the plan it names.
///
/// Refused rather than defaulted, field by field: a missing label, a venue
/// other than [`SIMULATED_VENUE`], a window past [`MAXIMUM_VALIDITY_SECS`],
/// a sequence of zero, a strategy the named plan does not contain, and any
/// field this command does not read — an unread field in a file that signs
/// capital is a term somebody believes is in force and is not.
pub fn load_fixture(path: &Path) -> Result<Fixture> {
    let text = read_bounded(path, "fixture")?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        Error::schema(format!(
            "the fixture at {} is not JSON: {error}",
            path.display()
        ))
    })?;
    let Value::Object(fields) = value else {
        return Err(Error::schema(format!(
            "the fixture at {} is not a JSON object",
            path.display()
        )));
    };
    if let Some(unknown) = fields
        .keys()
        .find(|key| !FIXTURE_FIELDS.contains(&key.as_str()))
    {
        return Err(Error::schema(format!(
            "the fixture at {} carries {unknown:?}, which this command does not read; remove it \
             rather than believe it is in force",
            path.display()
        )));
    }

    let label = text_field(&fields, "label", path).map_err(|_| {
        Error::denied(format!(
            "the fixture at {} does not carry \"label\": {FIXTURE_SIGNATORY:?}. Only a file \
             labelled as the grant fixture is signed",
            path.display()
        ))
    })?;
    if label != FIXTURE_SIGNATORY {
        return Err(Error::denied(format!(
            "the fixture at {} is labelled {label:?}, not {FIXTURE_SIGNATORY:?}. Only a file \
             labelled as the grant fixture is signed",
            path.display()
        )));
    }

    let cell = name_field(&fields, "cell", path)?;
    let region = name_field(&fields, "region", path)?;
    let strategy = text_field(&fields, "strategy", path)?;
    let venue = text_field(&fields, "venue", path)?;
    if venue != SIMULATED_VENUE {
        return Err(Error::denied(format!(
            "the fixture at {} names venue {venue:?}; a fixture grant names the simulated venue \
             {SIMULATED_VENUE:?} and nothing else",
            path.display()
        )));
    }
    let gross_limit = decimal_field(&fields, "gross_limit", path)?;
    let order_limit = decimal_field(&fields, "order_limit", path)?;
    let loss_limit = decimal_field(&fields, "loss_limit", path)?;

    let valid_for_secs = integer_field(&fields, "valid_for_secs", path)?;
    let valid_for_secs = i64::try_from(valid_for_secs)
        .ok()
        .filter(|secs| (1..=MAXIMUM_VALIDITY_SECS).contains(secs))
        .ok_or_else(|| {
            Error::invalid(format!(
                "the fixture at {} is valid for {valid_for_secs} seconds; a fixture grant is \
                 valid for between 1 and {MAXIMUM_VALIDITY_SECS}",
                path.display()
            ))
        })?;
    let policy_sequence = integer_field(&fields, "policy_sequence", path)?;
    if policy_sequence == 0 {
        return Err(Error::invalid(format!(
            "the fixture at {} has policy_sequence 0; a cell refuses any sequence at or below \
             the last it applied, and every cell starts having applied none above zero",
            path.display()
        )));
    }
    let partition = integer_field(&fields, "partition", path)?;
    let partition = u32::try_from(partition).map_err(|_| {
        Error::invalid(format!(
            "the fixture at {} names partition {partition}, which is not a partition number",
            path.display()
        ))
    })?;

    let plan_path = text_field(&fields, "plan", path)?;
    let plan_path = path
        .parent()
        .map_or_else(|| PathBuf::from(&plan_path), |dir| dir.join(&plan_path));
    let plan = digest_plan(&plan_path, &strategy)?;

    Ok(Fixture {
        cell,
        region,
        strategy,
        venue,
        gross_limit,
        order_limit,
        loss_limit,
        valid_for_secs,
        policy_sequence,
        partition,
        plan,
    })
}

/// The plan's digest and strategy count, refusing a plan that does not
/// contain the strategy the grant funds — a grant for a strategy the cell
/// will never run is capital nobody can use, and a payload naming it would
/// read as the slice being wired when it is not.
fn digest_plan(path: &Path, strategy: &str) -> Result<PlanDigest> {
    let text = read_bounded(path, "plan")?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        Error::schema(format!(
            "the plan at {} is not JSON: {error}",
            path.display()
        ))
    })?;
    let strategies = value
        .get("strategies")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Error::schema(format!(
                "the plan at {} has no \"strategies\" array",
                path.display()
            ))
        })?;
    if !strategies
        .iter()
        .any(|entry| entry.get("id").and_then(Value::as_str) == Some(strategy))
    {
        return Err(Error::invalid(format!(
            "the plan at {} does not contain strategy {strategy:?}, which the fixture grants \
             capital to",
            path.display()
        )));
    }
    let count = u64::try_from(strategies.len()).map_err(|_| {
        Error::invalid(format!(
            "the plan at {} has more strategies than can be counted",
            path.display()
        ))
    })?;
    Ok(PlanDigest {
        // The digest `qip_edge_node::strategies::StrategyPlan::digest_of`
        // takes: SHA-256 of the file's bytes, as the node will read them.
        digest: sha256_hex(text.as_bytes()),
        strategies: count,
    })
}

fn read_bounded(path: &Path, what: &str) -> Result<String> {
    let file = std::fs::File::open(path).map_err(|error| {
        Error::invalid(format!(
            "the {what} at {} could not be opened: {error}",
            path.display()
        ))
    })?;
    let mut text = String::new();
    // One byte past the bound, so a file of exactly the bound is read whole
    // and a longer one is detected rather than silently truncated.
    file.take(MAXIMUM_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| {
            Error::invalid(format!(
                "the {what} at {} could not be read as text: {error}",
                path.display()
            ))
        })?;
    if u64::try_from(text.len()).map_or(true, |length| length > MAXIMUM_FILE_BYTES) {
        return Err(Error::invalid(format!(
            "the {what} at {} is larger than {MAXIMUM_FILE_BYTES} bytes, which is not the file \
             that was meant",
            path.display()
        )));
    }
    Ok(text)
}

fn text_field(fields: &Map<String, Value>, name: &str, path: &Path) -> Result<String> {
    fields
        .get(name)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            Error::schema(format!(
                "the fixture at {} needs {name:?} as a non-empty string",
                path.display()
            ))
        })
}

/// A cell or region name: it becomes part of a stream name and a signing
/// string, so it is held to lower-case letters, digits and hyphens.
fn name_field(fields: &Map<String, Value>, name: &str, path: &Path) -> Result<String> {
    let text = text_field(fields, name, path)?;
    let well_formed = text.len() <= 63
        && text
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !well_formed {
        return Err(Error::schema(format!(
            "the fixture at {} names {name} {text:?}; a {name} is up to 63 lower-case letters, \
             digits and hyphens",
            path.display()
        )));
    }
    Ok(text)
}

fn decimal_field(fields: &Map<String, Value>, name: &str, path: &Path) -> Result<Decimal> {
    let text = text_field(fields, name, path)?;
    Decimal::parse(&text).ok_or_else(|| {
        Error::schema(format!(
            "the fixture at {} gives {name} as {text:?}, which is not a decimal. Money is \
             written as a string so it is never read through a float",
            path.display()
        ))
    })
}

fn integer_field(fields: &Map<String, Value>, name: &str, path: &Path) -> Result<u64> {
    fields.get(name).and_then(Value::as_u64).ok_or_else(|| {
        Error::schema(format!(
            "the fixture at {} needs {name:?} as a non-negative integer",
            path.display()
        ))
    })
}

/// The fixture key, read from the file [`KEY_VARIABLE`]`_FILE` names.
///
/// No `Clone`, and a `Debug` that prints nothing of the value, following
/// `BearerToken`: a `{:?}` in an error path is the ordinary way a key ends
/// up in a log.
pub struct FixtureKey {
    bytes: Vec<u8>,
}

impl FixtureKey {
    /// Accept key material the caller has already read, refusing one that
    /// is not labelled as a fixture key or is too short to be one. The
    /// refusal never contains the material.
    pub fn new(material: &str) -> Result<Self> {
        let Some(secret) = material.strip_prefix(FIXTURE_KEY_LABEL) else {
            return Err(Error::denied(format!(
                "the key {KEY_VARIABLE}{} names does not begin with {FIXTURE_KEY_LABEL:?}. Only \
                 a key labelled as a fixture key signs a fixture grant, so a production envelope \
                 key mounted here by mistake is refused rather than used",
                qip_core::secret::FILE_SUFFIX
            )));
        };
        if secret.len() < MINIMUM_KEY_MATERIAL {
            return Err(Error::denied(format!(
                "the fixture key carries fewer than {MINIMUM_KEY_MATERIAL} bytes after its label; \
                 generate one rather than type one"
            )));
        }
        Ok(Self {
            bytes: material.as_bytes().to_vec(),
        })
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl fmt::Debug for FixtureKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FixtureKey(<redacted>)")
    }
}

/// The `_FILE` path for `variable`, refusing the direct form.
///
/// Refused rather than ignored: an operator who exported the key directly
/// believes it is in use, and silently reading the file instead would sign
/// with a key they did not think was in play.
fn file_only(environment: &Environment, variable: &str) -> Result<Option<String>> {
    if environment.variable(variable).is_some() {
        return Err(Error::denied(format!(
            "{variable} is set directly. This command reads it only from the file \
             {variable}{} names: a credential in the environment is in /proc/<pid>/environ and \
             every child process. Unset {variable}",
            qip_core::secret::FILE_SUFFIX
        )));
    }
    Ok(environment.variable(&format!("{variable}{}", qip_core::secret::FILE_SUFFIX)))
}

/// The fixture key, through `qip_core::secret`'s `_FILE` rule only.
pub fn read_key(environment: &Environment) -> Result<FixtureKey> {
    let path = file_only(environment, KEY_VARIABLE)?;
    let Some(material) = qip_core::secret::resolve_from(KEY_VARIABLE, None, path)? else {
        return Err(Error::invalid(format!(
            "no fixture key: set {KEY_VARIABLE}{} to a file beginning {FIXTURE_KEY_LABEL:?}",
            qip_core::secret::FILE_SUFFIX
        )));
    };
    FixtureKey::new(&material)
}

/// The release controller's bearer token, through the `_FILE` rule only.
pub fn read_token(environment: &Environment) -> Result<BearerToken> {
    let path = file_only(environment, TOKEN_VARIABLE)?;
    BearerToken::resolve(TOKEN_VARIABLE, None, path)
}

/// What the command signed, before it is published.
#[derive(Clone, Debug, PartialEq)]
pub struct SignedControl {
    pub grant: CapitalEnvelope,
    pub policy: PolicyPayload,
    /// The grant's frame, then the payload's, as the cell's downlinks decode
    /// them. Grant first: the payload's manifest names the grant, and a cell
    /// applies capital before policy.
    pub frames: Vec<AnyEvent>,
}

/// Sign the fixture's grant and a payload naming the plan and the grant, at
/// `now`, with the fixture key — through the same construction the cell
/// verifies with.
pub fn sign(fixture: &Fixture, key: &FixtureKey, now: Timestamp) -> Result<SignedControl> {
    let expires_at = now.saturating_add(Duration::from_secs(fixture.valid_for_secs));
    let build = |signature: &str| {
        CapitalEnvelope::new(
            StrategyId::new(&fixture.strategy),
            &fixture.cell,
            fixture.gross_limit,
            fixture.order_limit,
            fixture.loss_limit,
            vec![VenueId::new(&fixture.venue)],
            now,
            expires_at,
            FIXTURE_SIGNATORY,
            signature,
        )
    };
    let unsigned = build("")?;
    let grant = build(&sign_payload(key.bytes(), &unsigned.signing_payload()))?;

    let mut policy = PolicyPayload::unproduced(fixture.policy_sequence, &fixture.cell, now);
    // The payload serves as long as the grant does, so the slice's share is
    // not narrowed to nothing five minutes into a session the grant still
    // funds.
    policy.valid_for = Duration::from_secs(fixture.valid_for_secs);
    policy.compiled_plan = Slot::produced(fixture.plan.clone(), now);
    policy.capital_grants = Slot::produced(
        GrantManifest {
            live_grants: vec![grant.signature().to_string()],
        },
        now,
    );
    let policy = policy.signed(key.bytes())?;

    let frames = vec![
        frame(CapitalGrantFrame(grant.clone()), "GRANT", now)?,
        frame(PolicyFrame(policy.clone()), "POLICY", now)?,
    ];
    Ok(SignedControl {
        grant,
        policy,
        frames,
    })
}

/// One body framed for the wire, its lineage naming the fixture.
///
/// The ids are derived from the body's own idempotency key, as the mesh's
/// dispatcher derives them, so re-running the command with the same fixture
/// at the same instant produces the same frame rather than a second one.
fn frame<T: EventBody>(body: T, kind: &str, at: Timestamp) -> Result<AnyEvent> {
    let key = body.idempotency_key().ok_or_else(|| {
        Error::invalid(format!(
            "a {kind} frame has no idempotency key, so a redelivery could not be recognised"
        ))
    })?;
    let digest = sha256_hex(key.as_bytes());
    Envelope::new(
        EventId::from_string(format!("EVT{kind}{digest}")),
        at,
        at,
        Lineage::root(
            CorrelationId::from_string(format!("COR{kind}{digest}")),
            FIXTURE_SIGNATORY,
        ),
        body,
    )
    .erase()
}

/// Publish the signed frames to the fixture's P0 stream as the release
/// controller, as one batch, at the quorum profile a P0 stream requires.
///
/// One batch so the grant and the payload whose manifest names it land
/// together or not at all: a cell that received the payload without the
/// grant would sum its share over a grant it never verified.
pub fn publish(
    signed: &SignedControl,
    fixture: &Fixture,
    transport: Box<dyn FabricTransport + Send>,
    environment: &Environment,
) -> Result<ProduceAck> {
    let mut producer = Producer::new(ProducerConfig {
        transport,
        stream: fixture.stream(),
        partition: fixture.partition,
        producer_id: RELEASE_CONTROLLER.to_string(),
        ack_profile: AckProfile::Quorum,
        retry_policy: RetryPolicy::default(),
        breaker_policy: BreakerPolicy::default(),
        clock: environment.clock(),
        sleeper: environment.sleeper(),
        retry_seed: RETRY_SEED,
        breaker_seed: BREAKER_SEED,
        timeouts: Timeouts::default(),
    })?;
    producer.init()?;
    let records = signed
        .frames
        .iter()
        .map(|frame| Record::from_any_event(frame, PayloadCodec::CanonicalJson))
        .collect::<Result<Vec<_>>>()?;
    let batch = Batch::new(
        MessageType::Data,
        UNREGISTERED_SCHEMA_ID,
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        records,
    )?;
    producer.send(batch)
}

/// The production transport to a checked loopback peer.
///
/// MERGE NOTE (SLICE-34): on the integration branch `HttpTransport::new`
/// takes the identity as its second argument and sends it as
/// `Authorization: Bearer <token>` on every call. This base predates that
/// change, so the token is dropped here and the call below must become
/// `HttpTransport::new(format!("http://{peer}"), identity)` at merge. Until
/// it does, a broker enforcing identity refuses every publish from this
/// command, which fails closed.
pub fn http_transport(peer: SocketAddr, identity: BearerToken) -> Box<dyn FabricTransport + Send> {
    drop(identity);
    Box::new(HttpTransport::new(format!("http://{peer}")))
}

/// `qip event-fabric grant --fixture <path> --peer <addr>`.
///
/// The order is the point. The arguments are refused before anything is
/// read; the peer is refused before the key is read, so key material is
/// never touched on a run that could not have gone anywhere safe; and
/// nothing is signed until the fixture, the key and the token have all been
/// accepted.
pub fn run(arguments: &[String], environment: &Environment) -> Result<Outcome> {
    let arguments = parse_arguments(arguments)?;
    let peer = loopback_peer(&arguments.peer)?;
    let fixture = load_fixture(&arguments.fixture)?;
    let key = read_key(environment)?;
    let token = read_token(environment)?;

    let now = environment.clock().now();
    let signed = sign(&fixture, &key, now)?;
    let ack = publish(
        &signed,
        &fixture,
        environment.connect(peer, token),
        environment,
    )?;

    let lines = vec![
        format!(
            "FIXTURE GRANT — signed as {FIXTURE_SIGNATORY} with a key labelled as a fixture key. \
             This is not how grants are issued (ADR 0100 §8); it reaches loopback only."
        ),
        format!("peer:      {peer} (loopback)"),
        format!(
            "stream:    {} partition {}, as {RELEASE_CONTROLLER}",
            fixture.stream(),
            fixture.partition
        ),
        format!(
            "grant:     {} at cell {}, venue {}, gross {}, order {}, loss {}, until {}",
            fixture.strategy,
            fixture.cell,
            fixture.venue,
            fixture.gross_limit,
            fixture.order_limit,
            fixture.loss_limit,
            signed.grant.expires_at().to_rfc3339()
        ),
        format!(
            "policy:    sequence {} for cell {}, plan {} ({} strategies), naming 1 grant",
            signed.policy.sequence, fixture.cell, fixture.plan.digest, fixture.plan.strategies
        ),
        format!(
            "published: base offset {}, archived through {}",
            ack.base_offset(),
            ack.archived_through()
        ),
        format!(
            "PAPER TRADING — the grant names the simulated venue {SIMULATED_VENUE} only and no \
             autonomy level changes."
        ),
    ];
    Ok(Outcome { lines, code: 0 })
}
