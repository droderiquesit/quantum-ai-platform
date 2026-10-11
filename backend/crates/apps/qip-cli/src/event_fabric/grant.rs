//! ADR 0100 §8: `qip event-fabric grant` — the labelled fixture, signed under
//! fixture-labelled keys, that pushes P0 control down to `qip-edge-node`.
//! SLICE-34.
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
//!   `"label": "fixture:qip-event-fabric-grant"`; and every key it signs
//!   under must say it is a fixture key — an asymmetric key version whose
//!   name begins [`FIXTURE_KEY_VERSION_PREFIX`] (for a Cloud KMS version,
//!   its `cryptoKeys/` name), or a shared key file beginning
//!   [`FIXTURE_KEY_LABEL`]. The last one is the fence that matters most: a
//!   production envelope or policy key configured here by mistake is refused
//!   before it signs anything, and a production cell's trust bundle never
//!   lists a key so named, so this command cannot mint a grant a production
//!   cell would accept.
//! - **Signed as the fixture.** Every envelope names [`FIXTURE_SIGNATORY`]
//!   as its signatory — the grant's approver and each frame's lineage
//!   producer — so a journal that records one shows where it came from
//!   without anybody's memory of how it got there.
//! - **Paper only.** The grant names [`SIMULATED_VENUE`] and nothing else,
//!   and nothing here can name an autonomy level: [`PolicyPayload`] has no
//!   field that could carry one.
//!
//! **A signature, not a MAC (ADR 0110).** The grant is signed under the
//! `envelope` class key and the payload under the `policy` class key, through
//! [`qip_contracts::trust::ArtefactSigner`] — the same signing message the
//! central plane uses, verified by the cell against its trust bundle. Which
//! signer is read from `QIP_ARTEFACT_SIGNER`, in the API's configuration
//! shape (`qip_api::signing`): `kms` signs through Cloud KMS with keys that
//! never enter this process; `local` signs from a keys file and is refused
//! unless `QIP_DEPLOYMENT=local`. Each signer is proven against the trust
//! bundle before anything is signed, so a key ring and a bundle that disagree
//! stop the command instead of publishing a grant every cell refuses.
//!
//! Unset, the command falls back to the shared HMAC fixture key the cell
//! verified before ADR 0110 — **only where `QIP_DEPLOYMENT=local`**. That is
//! stricter than the API, which keeps the shared key outside `local` until
//! `qip_transport::trust::SHARED_KEY_REMOVED_AFTER` because deployed cells
//! still verify it; this command has no deployed caller to keep working, and
//! a key that verifies can also sign, so every holder could mint the grant.
//!
//! **Mutual TLS to the broker (ADR 0112).** The link is
//! `qip_transport::tls::LinkSecurity::from_lookup` over `QIP_DEPLOYMENT` and
//! the `QIP_TLS_*` variables: mutual TLS with this workload's certificate,
//! and plaintext only where `QIP_DEPLOYMENT=local` *and*
//! `QIP_TLS_MODE=plaintext` say so. The bearer token still rides on top, as
//! the broker's authorisation. Because the transport is opened to the
//! checked loopback *address* rather than the name, a TLS broker serving
//! this command must hold a certificate valid for that address.
//!
//! The shared key and the release controller's bearer token are read only
//! through `qip_core::secret`'s `_FILE` indirection. The direct variable is
//! refused rather than preferred: a key in the environment is a key in
//! `/proc/<pid>/environ` and every child process. No key, token or private
//! key is printed, logged or placed in an error; [`FixtureKey`]'s `Debug` is
//! redacted and it has no `Clone`, following `BearerToken`, and the local
//! keys file's parse errors name a position, never the text there.

use qip_api::signing::{
    ARTEFACT_SIGNER_VARIABLE, GOOGLE_TLS_ROOTS_VARIABLE, KMS_KEY_VERSIONS_VARIABLE,
    LOCAL_SIGNING_KEYS_VARIABLE, MetadataBearer, TRUST_BUNDLE_VARIABLE,
};
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::policy::{GrantManifest, PlanDigest, PolicyPayload, Slot};
use qip_contracts::signal::StrategyId;
use qip_contracts::trust::{ArtefactClass, ArtefactSigner, sign_artefact};
use qip_contracts::venue::VenueId;
use qip_core::deployment::{DEPLOYMENT_VARIABLE, Deployment};
use qip_core::error::{Error, Result};
use qip_core::hash::{from_hex, sha256_hex};
use qip_core::ids::EventId;
use qip_core::time::Duration;
use qip_core::{CorrelationId, Decimal, Lineage, Timestamp};
use qip_edge::envelope::sign_payload;
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::{AckProfile, QosClass};
use qip_events::{AnyEvent, Envelope, EventBody};
use qip_mesh::spine::{CapitalGrantFrame, PolicyFrame};
use qip_transport::TlsHttpClient;
use qip_transport::breaker::BreakerPolicy;
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::producer::{Producer, ProducerConfig};
use qip_transport::event_fabric::protocol::ProduceAck;
use qip_transport::event_fabric::transport::{FabricTransport, HttpTransport, Timeouts};
use qip_transport::kms::{BearerSource, KmsSigner, prove_signer};
use qip_transport::retry::RetryPolicy;
use qip_transport::tls::{self, LinkSecurity};
use qip_transport::trust::{BundleVerifier, LocalSigner};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Read;
use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

/// What the shared fixture key file must begin with.
///
/// Part of the key: the MAC is taken over the whole file, label included, so
/// a cell verifying a fixture grant is configured with a key that says what
/// it is wherever it is mounted. A key without it is refused here, which is
/// what keeps a production envelope key from ever signing a fixture.
pub const FIXTURE_KEY_LABEL: &str = "qip-fixture-key:";

/// What the name of every asymmetric key this command signs under must begin
/// with: a local key version's whole name, or a Cloud KMS version's
/// `cryptoKeys/<name>` segment.
///
/// The asymmetric form of [`FIXTURE_KEY_LABEL`], and for the same reason.
/// The key version is part of what is signed and part of what the cell's
/// trust bundle lists, so the label travels with every signature: a cell
/// verifies a fixture grant only if its bundle names a fixture key, and a
/// production envelope or policy key configured here by mistake is refused
/// before it is asked to sign. A hyphen and not a colon because a key-version
/// name may not contain a colon — the signature's text form delimits with one.
pub const FIXTURE_KEY_VERSION_PREFIX: &str = "qip-fixture-";

/// The two classes this command signs: the grant, then the payload naming it.
/// Halts are not among them — a fixture that could halt a cell is a control
/// path nobody reviewed — and model packs are signed only by the publish
/// workflow.
const SIGNED_HERE: [ArtefactClass; 2] = [ArtefactClass::Envelope, ArtefactClass::Policy];

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

/// The batch header's schema id: the P0 control class's own, from the one
/// table the broker registers every declared stream against (FABRIC-024).
/// This read `0` under the name `UNREGISTERED_SCHEMA_ID` while no broker
/// enforced a registry; a broker that does would have refused every grant
/// this command published had the two numbers ever drifted apart. Each
/// record's `AnyEvent` still names its own topic and schema version, and
/// that is what the cell's downlink checks before it decodes anything.
const CONTROL_BATCH_SCHEMA_ID: u32 = QosClass::P0Control.batch_schema_id();

/// The batch header's schema version, from the same table.
const BATCH_SCHEMA_VERSION: u32 = qip_events::event_fabric::policy::BATCH_SCHEMA_VERSION;

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

/// The local slice's one P0 control stream (SLICE-53's catalogue).
pub const LOCAL_CONTROL_STREAM: &str = "control.local";

impl Fixture {
    /// The P0 stream this fixture's control goes to: `control.local`, the one
    /// P0 stream the committed local catalogue declares
    /// (`infrastructure/event-fabric/streams.local.json`, SLICE-53) and the one
    /// the node's control consumer reads (SLICE-57's `CONTROL_STREAM`). It is
    /// not derived from the region. `control.<region>` named a stream no
    /// catalogue declared, so a grant published there would have been refused
    /// by the broker, or never read by the cell. The region still stamps the
    /// envelope; it just does not name the stream.
    pub fn stream(&self) -> String {
        LOCAL_CONTROL_STREAM.to_string()
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
pub(crate) fn file_only(environment: &Environment, variable: &str) -> Result<Option<String>> {
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

/// What the fixture is signed under.
pub enum FixtureSigner {
    /// ADR 0110: the grant under the `envelope` class key and the payload
    /// under the `policy` class key, each already proven against the trust
    /// bundle the cell verifies with.
    Asymmetric {
        envelope: Arc<dyn ArtefactSigner>,
        policy: Arc<dyn ArtefactSigner>,
    },
    /// The shared HMAC fixture key the cell verified before ADR 0110,
    /// admitted only in a `local` deployment.
    SharedKey(FixtureKey),
}

impl FixtureSigner {
    /// One line for the command's output. Names key versions, which are
    /// resource names and not secrets, and nothing of a key.
    pub fn describe(&self) -> String {
        match self {
            Self::Asymmetric { envelope, policy } => format!(
                "ECDSA P-256 (ADR 0110): envelope {}, policy {}",
                envelope.key_version(),
                policy.key_version()
            ),
            Self::SharedKey(_) => format!(
                "SHARED HMAC FIXTURE KEY — local deployment only; whoever can verify it can sign \
                 with it. Set {ARTEFACT_SIGNER_VARIABLE}=local or kms"
            ),
        }
    }
}

impl fmt::Debug for FixtureSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

/// Whether `key_version` is named as a fixture key: its `cryptoKeys/` name
/// for a Cloud KMS version, its whole name otherwise, begins
/// [`FIXTURE_KEY_VERSION_PREFIX`].
pub fn is_fixture_key_version(key_version: &str) -> bool {
    let name = key_version
        .split_once("/cryptoKeys/")
        .map_or(key_version, |(_, key)| key);
    name.starts_with(FIXTURE_KEY_VERSION_PREFIX)
}

fn require_fixture_key(class: ArtefactClass, key_version: &str) -> Result<()> {
    if is_fixture_key_version(key_version) {
        return Ok(());
    }
    Err(Error::denied(format!(
        "the {class} key version {key_version:?} is not named as a fixture key. Only a key whose \
         name (for Cloud KMS, its cryptoKeys/ name) begins {FIXTURE_KEY_VERSION_PREFIX:?} signs a \
         fixture grant, so a production {class} key configured here by mistake is refused before \
         it signs anything"
    )))
}

/// A path variable's value, trimmed, or `None` when unset or blank.
fn path_variable(environment: &Environment, variable: &str) -> Option<PathBuf> {
    environment
        .variable(variable)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn required_path(environment: &Environment, variable: &str, mode: &str) -> Result<PathBuf> {
    path_variable(environment, variable).ok_or_else(|| {
        Error::invalid(format!(
            "{ARTEFACT_SIGNER_VARIABLE}={mode} needs {variable}; without it nothing this command \
             signs can be checked against what the cell holds"
        ))
    })
}

/// A JSON object read from a small configuration file, every key of which
/// must name one of [`SIGNED_HERE`]. A parse failure names the position and
/// never the text there, because one of these files holds private keys.
fn class_map(path: &Path, what: &str) -> Result<Map<String, Value>> {
    let text = read_bounded(path, what)?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        Error::invalid(format!(
            "the {what} at {} does not parse as JSON (line {}, column {})",
            path.display(),
            error.line(),
            error.column()
        ))
    })?;
    let Value::Object(entries) = value else {
        return Err(Error::invalid(format!(
            "the {what} at {} is not a JSON object keyed by class",
            path.display()
        )));
    };
    if let Some(unread) = entries.keys().find(|key| {
        !SIGNED_HERE
            .iter()
            .any(|class| class.as_str() == key.as_str())
    }) {
        return Err(Error::invalid(format!(
            "the {what} at {} names {unread:?}; this command signs only the envelope and policy \
             classes, and an entry it does not read is a key somebody believes is in use",
            path.display()
        )));
    }
    Ok(entries)
}

/// The local signers, from the keys file `QIP_LOCAL_SIGNING_KEYS_PATH`
/// names: the API's shape (`{"<class>": {"key_version", "pkcs8_hex"}}`),
/// holding exactly the two classes this command signs. Each key version's
/// fixture label is checked before its private key is parsed.
fn local_signers(path: &Path) -> Result<BTreeMap<ArtefactClass, Arc<dyn ArtefactSigner>>> {
    let entries = class_map(path, "local signing keys file")?;
    let mut signers: BTreeMap<ArtefactClass, Arc<dyn ArtefactSigner>> = BTreeMap::new();
    for class in SIGNED_HERE {
        let entry = entries
            .get(class.as_str())
            .and_then(Value::as_object)
            .ok_or_else(|| {
                Error::invalid(format!(
                    "the local signing keys file has no {class} entry of the form \
                     {{\"key_version\", \"pkcs8_hex\"}}"
                ))
            })?;
        if let Some(unread) = entry
            .keys()
            .find(|key| !matches!(key.as_str(), "key_version" | "pkcs8_hex"))
        {
            return Err(Error::invalid(format!(
                "the local {class} key carries {unread:?}, which this command does not read"
            )));
        }
        let text = |field: &str| {
            entry
                .get(field)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    Error::invalid(format!(
                        "the local {class} key needs {field:?} as a non-empty string"
                    ))
                })
        };
        let key_version = text("key_version")?;
        require_fixture_key(class, key_version)?;
        let pkcs8 = from_hex(text("pkcs8_hex")?)
            .ok_or_else(|| Error::invalid(format!("the local {class} key is not hexadecimal")))?;
        signers.insert(
            class,
            Arc::new(LocalSigner::from_pkcs8(class, key_version, &pkcs8)?),
        );
    }
    Ok(signers)
}

/// The Cloud KMS key versions, from the map `QIP_KMS_KEY_VERSIONS_PATH`
/// names — the API's shape, holding exactly the two classes this command
/// signs. Every version's fixture label is checked here, before a signer is
/// built and before anything is sent to Google.
fn kms_versions(path: &Path) -> Result<BTreeMap<ArtefactClass, String>> {
    let entries = class_map(path, "KMS key-version map")?;
    let mut versions = BTreeMap::new();
    for class in SIGNED_HERE {
        let version = entries
            .get(class.as_str())
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                Error::invalid(format!(
                    "the KMS key-version map has no {class} entry; it maps envelope and policy \
                     each to a full cryptoKeyVersions resource name"
                ))
            })?;
        require_fixture_key(class, version)?;
        versions.insert(class, version.to_string());
    }
    Ok(versions)
}

/// The signer every artefact this command publishes goes out under.
///
/// The API's configuration (`qip_api::signing::configure`), narrowed to the
/// two classes a fixture signs and with the fixture-label fence added. Its
/// rules are the API's: `local` only in a `local` deployment, the trust
/// bundle required for either asymmetric posture, and every signer proven
/// against it before it is used. Unset is the shared fixture key, refused
/// outside `local` — see the module documentation for why that is stricter
/// than the API.
///
/// Also refused, before any file is read: the shared key's variable set
/// beside an asymmetric signer. Choosing one by precedence would hide that
/// the shared key's mount was never taken away — the reason `qip-edge-node`
/// refuses both of its roots at once.
pub fn read_signer(
    environment: &Environment,
    deployment: Deployment,
    now: Timestamp,
) -> Result<FixtureSigner> {
    let mode = environment
        .variable(ARTEFACT_SIGNER_VARIABLE)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let Some(mode) = mode else {
        if !deployment.permits_reproducible_keys() {
            return Err(Error::denied(format!(
                "{DEPLOYMENT_VARIABLE}={deployment} and no {ARTEFACT_SIGNER_VARIABLE} is set, so \
                 this command would sign with the shared HMAC fixture key — and a key that \
                 verifies can also sign, so every holder could mint the grant (ADR 0110). The \
                 shared key is admitted only where {DEPLOYMENT_VARIABLE}=local; set \
                 {ARTEFACT_SIGNER_VARIABLE}=kms"
            )));
        }
        return Ok(FixtureSigner::SharedKey(read_key(environment)?));
    };
    if !matches!(mode.as_str(), "kms" | "local") {
        return Err(Error::invalid(format!(
            "{ARTEFACT_SIGNER_VARIABLE}={mode:?} is not a signer; use `kms`, or `local` on a \
             developer machine, or leave it unset for the shared fixture key on one"
        )));
    }
    let shared_key_file = format!("{KEY_VARIABLE}{}", qip_core::secret::FILE_SUFFIX);
    if environment.variable(KEY_VARIABLE).is_some()
        || environment.variable(&shared_key_file).is_some()
    {
        return Err(Error::denied(format!(
            "{ARTEFACT_SIGNER_VARIABLE}={mode} and the shared fixture key ({shared_key_file}) are \
             both configured. A run signing asymmetrically must not also hold the shared key; \
             unset {shared_key_file}"
        )));
    }
    if mode == "local" && !deployment.permits_reproducible_keys() {
        return Err(Error::denied(format!(
            "{ARTEFACT_SIGNER_VARIABLE}=local in the {deployment} deployment; a private key on a \
             deployed disk is what ADR 0110 removes. Use `kms`"
        )));
    }
    let verifier =
        BundleVerifier::from_file(&required_path(environment, TRUST_BUNDLE_VARIABLE, &mode)?)?;
    let mut signers = if mode == "kms" {
        let versions = kms_versions(&required_path(
            environment,
            KMS_KEY_VERSIONS_VARIABLE,
            &mode,
        )?)?;
        let roots = read_bounded(
            &required_path(environment, GOOGLE_TLS_ROOTS_VARIABLE, &mode)?,
            "Google TLS roots",
        )?;
        let client = TlsHttpClient::new(
            KmsSigner::limits(),
            tls::server_authenticated_client(roots.as_bytes())?,
        );
        let bearer: Arc<dyn BearerSource> = Arc::new(MetadataBearer(
            qip_storage::gcp::auth::MetadataServerTokens::new(environment.clock()),
        ));
        let mut signers: BTreeMap<ArtefactClass, Arc<dyn ArtefactSigner>> = BTreeMap::new();
        for (class, version) in versions {
            signers.insert(
                class,
                Arc::new(KmsSigner::new(
                    class,
                    version,
                    client.clone(),
                    Arc::clone(&bearer),
                    verifier.clone(),
                    environment.clock(),
                )?),
            );
        }
        signers
    } else {
        local_signers(&required_path(
            environment,
            LOCAL_SIGNING_KEYS_VARIABLE,
            &mode,
        )?)?
    };
    let mut take = |class: ArtefactClass| -> Result<Arc<dyn ArtefactSigner>> {
        let signer = signers
            .remove(&class)
            .ok_or_else(|| Error::invalid(format!("no {class} signer was configured")))?;
        prove_signer(signer.as_ref(), &verifier, now).map_err(|error| {
            Error::denied(format!(
                "the {class} signer ({}) does not verify under the trust bundle \
                 {TRUST_BUNDLE_VARIABLE} names: {}. A grant signed under it would be refused by \
                 every cell holding that bundle, so nothing is signed",
                signer.key_version(),
                error.message()
            ))
        })?;
        Ok(signer)
    };
    let envelope = take(ArtefactClass::Envelope)?;
    let policy = take(ArtefactClass::Policy)?;
    Ok(FixtureSigner::Asymmetric { envelope, policy })
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
/// `now`, under `signer` — through the signing message the cell verifies
/// with: [`sign_artefact`] over the envelope's signing payload and
/// [`PolicyPayload::signed_by`] for the payload, or, for the shared key, the
/// cell's own MAC construction.
pub fn sign(fixture: &Fixture, signer: &FixtureSigner, now: Timestamp) -> Result<SignedControl> {
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
    let signature = match signer {
        FixtureSigner::Asymmetric { envelope, .. } => sign_artefact(
            envelope.as_ref(),
            ArtefactClass::Envelope,
            unsigned.signing_payload().as_bytes(),
        )?,
        FixtureSigner::SharedKey(key) => sign_payload(key.bytes(), &unsigned.signing_payload()),
    };
    let grant = build(&signature)?;

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
    let policy = match signer {
        FixtureSigner::Asymmetric {
            policy: policy_signer,
            ..
        } => policy.signed_by(policy_signer.as_ref())?,
        FixtureSigner::SharedKey(key) => policy.signed(key.bytes())?,
    };

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
        qos_class: QosClass::P0Control,
        ack_profile: AckProfile::Archived,
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
        CONTROL_BATCH_SCHEMA_ID,
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        records,
    )?;
    producer.send(batch)
}

/// A plaintext transport to a checked loopback peer, for `qip event-fabric
/// isolate` and `release` ([`super::admin`]), which have not moved to a
/// configured link. The grant does not use it: see [`link_transport`].
///
/// The token goes on every call as `Authorization: Bearer <token>`
/// (FABRIC-AUTH), so the broker can check what the caller may do.
pub fn http_transport(peer: SocketAddr, identity: BearerToken) -> Box<dyn FabricTransport + Send> {
    Box::new(HttpTransport::new(format!("http://{peer}"), identity))
}

/// The transport to a checked loopback peer over `link` (ADR 0112): `https`
/// with this workload's certificate when the link is mutual TLS, `http` only
/// for the plaintext a `local` deployment chose. Built by
/// [`HttpTransport::over`], the constructor `qip-edge-node`'s outbox uses, so
/// the scheme follows the link and a run configured for TLS cannot be handed
/// a plaintext URL.
///
/// Opened to the checked address, never the name, so the loopback fence
/// holds under TLS too — which is why the broker's certificate must be valid
/// for that address.
pub fn fabric_transport(
    link: &LinkSecurity,
    peer: SocketAddr,
    identity: BearerToken,
) -> Result<HttpTransport> {
    HttpTransport::over(link, &peer.to_string(), identity)
}

/// [`fabric_transport`], boxed: the production [`super::LinkConnector`].
pub fn link_transport(
    link: &LinkSecurity,
    peer: SocketAddr,
    identity: BearerToken,
) -> Result<Box<dyn FabricTransport + Send>> {
    Ok(Box::new(fabric_transport(link, peer, identity)?))
}

/// `qip event-fabric grant --fixture <path> --peer <addr>`.
///
/// The order is the point. The arguments are refused before anything is
/// read; the peer is refused before the deployment, the link or any key is
/// read, so key material is never touched on a run that could not have gone
/// anywhere safe; the link is decided — its certificate loaded — before the
/// signer, so a run that cannot reach the broker securely signs nothing; and
/// nothing is signed until the fixture, the signer (proven against the trust
/// bundle) and the token have all been accepted.
pub fn run(arguments: &[String], environment: &Environment) -> Result<Outcome> {
    let arguments = parse_arguments(arguments)?;
    let peer = loopback_peer(&arguments.peer)?;
    let fixture = load_fixture(&arguments.fixture)?;
    // Fail closed: `Deployment::parse` refuses an absent value, so a run that
    // forgot to say where it is cannot reach the plaintext link or the shared
    // key only a workstation may choose.
    let deployment = Deployment::parse(environment.variable(DEPLOYMENT_VARIABLE).as_deref())?;
    let link = LinkSecurity::from_lookup(deployment, &|name| environment.variable(name))?;
    let now = environment.clock().now();
    let signer = read_signer(environment, deployment, now)?;
    let token = read_token(environment)?;

    let signed = sign(&fixture, &signer, now)?;
    let ack = publish(
        &signed,
        &fixture,
        environment.connect_over(&link, peer, token)?,
        environment,
    )?;

    let lines = vec![
        format!(
            "FIXTURE GRANT — signed as {FIXTURE_SIGNATORY} under keys labelled as fixture keys. \
             This is not how grants are issued (ADR 0100 §8); it reaches loopback only."
        ),
        format!("signer:    {}", signer.describe()),
        format!("peer:      {peer} (loopback), {deployment} deployment"),
        format!("link:      {}", link.describe()),
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
