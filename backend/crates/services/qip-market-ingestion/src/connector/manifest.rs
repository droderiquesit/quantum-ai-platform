//! The manifest: everything about a source that is configuration, not code.
//!
//! Adding a data source used to mean edits in several crates — a config
//! struct here, a descriptor there, a poll interval in a deployment file. A
//! manifest is one JSON document that holds all of it, so a new source is a
//! file plus a small adapter and nothing else has to change.
//!
//! # Why JSON and not YAML
//!
//! The dependency policy (`scripts/check-dependencies.sh`) permits `serde` and
//! `serde_json` and nothing else. A YAML parser is a dependency; a hand-rolled
//! one would be a YAML parser written by us guarding what the platform is
//! allowed to fetch. JSON is what is already in the build.
//!
//! # Why unknown fields are refused
//!
//! Every struct here is `deny_unknown_fields`. A manifest that wrote
//! `poll_interval_millis` instead of `poll_interval_ms` would otherwise parse,
//! take the default, and poll at a cadence nobody chose — a rate-limit ban
//! discovered in production instead of a parse error discovered in review.
//!
//! # Why a credential cannot be written here
//!
//! [`AuthSpec`] has no field capable of holding a credential value. It holds a
//! [`SecretRef`], which is the *name* of a deployment variable, and
//! [`SecretRef::new`] refuses any name that is not `SCREAMING_SNAKE_CASE` —
//! which is exactly the shape a pasted API key (`sk-live-9f2a…`) is not. A
//! manifest is checked into git and printed in support tickets; the failure
//! prevented is a live key in both. A source that reads two headers — an
//! account id beside a secret key — names a [`CompanionHeader`], which holds
//! a second [`SecretRef`] under the same rule and nothing else.

use qip_core::Duration;
use qip_core::error::{Error, Result};
use qip_financial::quality::LicensingClass;
use qip_transport::RetryPolicy;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The broad market a source reports on.
///
/// Coarse on purpose: it decides which downstream contracts a record is
/// checked against and which desk is paged, not what the record means.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetClass {
    Equities,
    FixedIncome,
    Fx,
    Crypto,
    Commodities,
    Rates,
    Credit,
    Macro,
    Alternative,
    CrossAsset,
}

/// Where the source's coverage sits, for entitlement and residency questions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    Global,
    NorthAmerica,
    Europe,
    Apac,
    LatinAmerica,
    MiddleEastAfrica,
}

/// How the source delivers.
///
/// This is the field that decides whether a connector may be driven by
/// `poll(until)` directly or has to buffer. An author who is not told which
/// they are writing writes one that blocks the ingestion loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// Request/response over HTTP, one batch per call.
    Rest,
    /// A long-lived subscription that pushes. The connector buffers arrivals
    /// and drains them on demand; the caller still owns the clock.
    WebSocket,
    /// A snapshot endpoint with no cursor: every call returns "now".
    Poll,
    /// A file or object drop, listed and read.
    File,
}

impl Protocol {
    /// Whether the caller's clock drives the fetch, or the source's does.
    pub const fn is_pull(self) -> bool {
        !matches!(self, Self::WebSocket)
    }

    /// Whether a connector over this protocol must buffer arrivals rather
    /// than fetch them inside the call.
    pub const fn requires_buffering(self) -> bool {
        matches!(self, Self::WebSocket)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rest => "rest",
            Self::WebSocket => "websocket",
            Self::Poll => "poll",
            Self::File => "file",
        }
    }
}

/// A `major.minor` version of the payload shape a connector was written for.
///
/// Two numbers rather than one because the two failures are different: a new
/// optional field is a minor bump a connector survives, and a renamed or
/// retyped field is a major bump that would make the connector decode the
/// wrong thing. [`Self::admits`] is what turns the second into a refusal at
/// the boundary instead of a silently mis-decoded record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SchemaVersion {
    major: u32,
    minor: u32,
}

impl SchemaVersion {
    pub const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }

    pub const fn major(self) -> u32 {
        self.major
    }

    pub const fn minor(self) -> u32 {
        self.minor
    }

    /// Whether a payload declaring `observed` may be decoded by a connector
    /// written against `self`.
    ///
    /// Same major, and the observed minor at least what the connector needs.
    /// A *newer* minor is admitted: the source adding a field is not a fault
    /// and must not stop the feed.
    pub const fn admits(self, observed: Self) -> bool {
        self.major == observed.major && observed.minor >= self.minor
    }

    pub fn parse(text: &str) -> Result<Self> {
        let (major, minor) = text.split_once('.').ok_or_else(|| {
            Error::invalid(format!(
                "the schema version {text:?} is not `major.minor`; one number cannot say whether \
                 a change is compatible"
            ))
        })?;
        let major = major.trim().parse::<u32>().map_err(|_| {
            Error::invalid(format!(
                "the major part of the schema version {text:?} is not a number"
            ))
        })?;
        let minor = minor.trim().parse::<u32>().map_err(|_| {
            Error::invalid(format!(
                "the minor part of the schema version {text:?} is not a number"
            ))
        })?;
        Ok(Self { major, minor })
    }
}

impl std::fmt::Display for SchemaVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

impl TryFrom<String> for SchemaVersion {
    type Error = String;

    fn try_from(value: String) -> std::result::Result<Self, String> {
        Self::parse(&value).map_err(|error| error.message().to_string())
    }
}

impl From<SchemaVersion> for String {
    fn from(value: SchemaVersion) -> Self {
        value.to_string()
    }
}

/// The JSON type a required field must carry.
///
/// `DecimalString` and `Number` are separate because a price sent as a JSON
/// float has already lost precision by the time this code sees it; a source
/// that switches from `"101.75"` to `101.75` has made a breaking change and
/// this is what notices.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    String,
    Number,
    /// A number written as a string, which is how an exact price travels.
    DecimalString,
    /// An RFC 3339 instant, written as a string.
    Timestamp,
    Bool,
    Object,
    Array,
}

impl FieldKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::DecimalString => "decimal string",
            Self::Timestamp => "RFC 3339 timestamp",
            Self::Bool => "bool",
            Self::Object => "object",
            Self::Array => "array",
        }
    }
}

/// One field the payload must carry, addressed by a dotted path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldSpec {
    /// Dotted path from the payload root, e.g. `rates.EUR`. A path segment
    /// that is an integer indexes an array.
    pub path: String,
    pub kind: FieldKind,
}

impl FieldSpec {
    pub fn new(path: impl Into<String>, kind: FieldKind) -> Self {
        Self {
            path: path.into(),
            kind,
        }
    }
}

/// What to do about a field the contract does not name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnknownFieldPolicy {
    /// A source adding a field is not a fault. The default.
    #[default]
    Ignore,
    /// For a source whose additions have historically been breaking.
    Quarantine,
}

/// What a well-formed payload from this source looks like.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaContract {
    pub version: SchemaVersion,
    /// Fields whose absence or wrong type means the payload cannot be decoded.
    #[serde(default)]
    pub required_fields: Vec<FieldSpec>,
    #[serde(default)]
    pub unknown_fields: UnknownFieldPolicy,
}

/// How a credential is presented, never what it is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthScheme {
    /// A genuinely open endpoint. Not the same as "we have not configured it
    /// yet": an unconfigured credential is a `header` or `bearer` scheme whose
    /// variable is unset, which fails at connect.
    #[default]
    None,
    /// The credential travels in a named header.
    Header,
    /// `authorization: Bearer <credential>`.
    Bearer,
}

/// The name of a deployment secret. Never its value.
///
/// [`Self::new`] refuses anything that is not `SCREAMING_SNAKE_CASE`, which is
/// what stops a pasted key from being written where a variable name belongs:
/// `sk-live-9f2a` and an AWS access-key id followed by `/abc` both fail, and
/// the refusal arrives at manifest load rather than after the key is in git
/// history. The key id is described rather than spelled because a literal
/// one — even AWS's published example — trips `./scripts/check-secrets.sh`
/// on every run, and a scanner that always reports something is a scanner
/// people stop reading.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretRef {
    variable: String,
}

impl SecretRef {
    /// The shortest name that is plausibly a variable and not a key fragment.
    const MIN_LENGTH: usize = 4;

    pub fn new(variable: impl Into<String>) -> Result<Self> {
        let variable = variable.into();
        let reference = Self { variable };
        reference.validate()?;
        Ok(reference)
    }

    pub fn variable(&self) -> &str {
        &self.variable
    }

    pub fn validate(&self) -> Result<()> {
        let variable = self.variable.trim();
        if variable.len() < Self::MIN_LENGTH {
            return Err(Error::invalid(format!(
                "the secret reference {variable:?} is too short to be a deployment variable name"
            )));
        }
        let first_is_letter = variable
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase());
        let rest_is_name = variable
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !first_is_letter || !rest_is_name {
            return Err(Error::invalid(format!(
                "{variable:?} is not a deployment variable name. A manifest names the variable \
                 the credential is read from and never carries the credential itself; only \
                 A-Z, 0-9 and _ are accepted, so a pasted key cannot be written here"
            )));
        }
        Ok(())
    }

    /// Read the credential the deployment supplied under this name.
    ///
    /// Delegates to [`qip_core::secret_from_environment`], so the variable and
    /// the `_FILE` variant behave the same here as everywhere else in the
    /// platform — including refusing both being set at once.
    pub fn resolve(&self) -> Result<Option<String>> {
        qip_core::secret_from_environment(&self.variable)
    }

    /// Read the credential from a lookup that stands in for the environment.
    ///
    /// The same rule as [`Self::resolve`] — the variable, its `_FILE` variant,
    /// both refused — applied to whatever `environment` answers for a name.
    /// This exists because the process environment cannot be written in a
    /// test (`set_var` is `unsafe` since the 2024 edition and this workspace
    /// forbids `unsafe`), and a resolver that could only be exercised against
    /// the real environment would be one whose `_FILE` path nothing proved.
    pub fn resolve_with(
        &self,
        environment: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Option<String>> {
        let file_variable = format!("{}{}", self.variable, qip_core::secret::FILE_SUFFIX);
        qip_core::secret::resolve_from(
            &self.variable,
            environment(&self.variable),
            environment(&file_variable),
        )
    }
}

/// How a credential reaches a request; the function type every resolver has.
///
/// [`SecretRef::resolve`] is one, reading the deployment's environment; a
/// test supplies another over a map and two files. The transport and
/// [`SourceManifest::missing_configuration_with`] take one of these rather
/// than reading the environment themselves, so the two-header path can be
/// proven end to end without a socket to a vendor or a variable set on the
/// machine running the build.
pub type SecretResolver<'a> = &'a dyn Fn(&SecretRef) -> Result<Option<String>>;

/// Headers a credential may not be routed to.
///
/// The first four are the ones `qip_transport::HttpRequest::with_header`
/// silently drops because the client writes them itself; a manifest routing a
/// secret to `host` would send no credential at all and answer every 401 with
/// a shrug. The rest are content-negotiation headers the transport sets or a
/// vendor reads for something other than identity; a secret sent as `accept`
/// is a credential in a header every proxy on the path logs as routine.
const NON_CREDENTIAL_HEADERS: [&str; 8] = [
    "host",
    "content-length",
    "connection",
    "transfer-encoding",
    "accept",
    "accept-encoding",
    "content-type",
    "user-agent",
];

/// The rule every credential header name is held to.
///
/// Lower-case, printable ASCII, no colon, and not one of the headers the
/// client owns. The colon and space rule is about the wire: a name that
/// contained either would end the header and let the rest be read as another.
fn validate_credential_header(header: &str) -> Result<()> {
    if header.is_empty() {
        return Err(Error::invalid(
            "a credential header with no name; the credential is never put in the URL, because \
             a URL is written to every access log on the path",
        ));
    }
    if !header
        .chars()
        .all(|c| c.is_ascii_graphic() && c != ':' && !c.is_ascii_uppercase())
    {
        return Err(Error::invalid(format!(
            "{header:?} is not a usable header name: it must be lower-case ASCII with no colon \
             or space, or it would end the header and let the rest be read as another"
        )));
    }
    if NON_CREDENTIAL_HEADERS.contains(&header) {
        return Err(Error::invalid(format!(
            "`{header}` is not a credential header: the transport writes it itself, so a secret \
             routed there is either dropped before the socket or sent where every proxy logs it \
             as routine. Name the header the vendor reads the credential from"
        )));
    }
    Ok(())
}

/// A second credential header, for a source that identifies the account in
/// one header and authenticates it in another.
///
/// Alpaca is the case in hand: `apca-api-key-id` and `apca-api-secret-key`,
/// and a request carrying only the second answers 401. The failure prevented
/// by making this a named field rather than a free list is a manifest that
/// declares three headers, or the same one twice, or one that routes the key
/// id to `accept`; each is a shape this type cannot take or
/// [`AuthSpec::validate`] refuses by name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompanionHeader {
    /// The header the second credential travels in.
    pub header: String,
    /// Where its value is read from: a name, never a value, under the same
    /// rule as [`AuthSpec::secret`].
    pub secret: SecretRef,
}

impl CompanionHeader {
    pub fn new(header: impl Into<String>, secret: SecretRef) -> Self {
        Self {
            header: header.into(),
            secret,
        }
    }
}

/// How the source authenticates a request.
///
/// A struct rather than an enum carrying a value, so that there is no shape of
/// this type in which a credential can appear.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthSpec {
    #[serde(default)]
    pub scheme: AuthScheme,
    /// The header the credential travels in, for [`AuthScheme::Header`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretRef>,
    /// A second header the source requires beside [`Self::header`], for
    /// [`AuthScheme::Header`] only. Absent in every manifest written before
    /// it existed, which is why it defaults rather than being required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub companion: Option<CompanionHeader>,
}

impl AuthSpec {
    /// A source that needs no credential.
    pub fn open() -> Self {
        Self::default()
    }

    /// A credential in a named header, read from a named deployment variable.
    pub fn header(header: impl Into<String>, secret: SecretRef) -> Self {
        Self {
            scheme: AuthScheme::Header,
            header: Some(header.into()),
            secret: Some(secret),
            companion: None,
        }
    }

    /// The same, with a second header the source reads beside the first.
    pub fn two_headers(
        header: impl Into<String>,
        secret: SecretRef,
        companion: CompanionHeader,
    ) -> Self {
        Self {
            scheme: AuthScheme::Header,
            header: Some(header.into()),
            secret: Some(secret),
            companion: Some(companion),
        }
    }

    /// Every secret this spec names, in the order the transport sends the
    /// headers they fill: the primary first, the companion second.
    pub fn secrets(&self) -> Vec<&SecretRef> {
        self.secret
            .iter()
            .chain(self.companion.iter().map(|companion| &companion.secret))
            .collect()
    }

    pub fn validate(&self) -> Result<()> {
        if let Some(secret) = &self.secret {
            secret.validate()?;
        }
        if let Some(companion) = &self.companion {
            companion.secret.validate()?;
        }
        match self.scheme {
            AuthScheme::None => {
                if self.secret.is_some() || self.header.is_some() || self.companion.is_some() {
                    return Err(Error::invalid(
                        "the auth scheme is `none` but a header or secret is named; an open \
                         endpoint that quietly carries a credential is one nobody audits",
                    ));
                }
            }
            AuthScheme::Header => {
                let header = self.header.as_deref().unwrap_or_default().trim();
                if header.is_empty() {
                    return Err(Error::invalid(
                        "the auth scheme is `header` but no header is named; the credential is \
                         never put in the URL, because a URL is written to every access log on \
                         the path",
                    ));
                }
                validate_credential_header(header)?;
                if self.secret.is_none() {
                    return Err(Error::invalid(
                        "the auth scheme is `header` but no secret variable is named",
                    ));
                }
                if let Some(companion) = &self.companion {
                    let second = companion.header.trim();
                    validate_credential_header(second)?;
                    if second == header {
                        return Err(Error::invalid(format!(
                            "`{header}` is named as both the credential header and its \
                             companion. Two values under one name reach the vendor as whichever \
                             its parser keeps, so the manifest would authenticate with a \
                             credential nobody chose; name the second header the vendor reads"
                        )));
                    }
                    if self.secrets().windows(2).any(|pair| pair[0] == pair[1]) {
                        return Err(Error::invalid(format!(
                            "`{header}` and `{second}` both read `{}`. One credential sent under \
                             two names means one of the two headers is carrying the wrong \
                             thing; name a variable for each",
                            companion.secret.variable()
                        )));
                    }
                }
            }
            AuthScheme::Bearer => {
                if self.header.is_some() {
                    return Err(Error::invalid(
                        "a bearer credential travels in `authorization`; naming a second header \
                         would send it twice or not at all",
                    ));
                }
                if self.companion.is_some() {
                    return Err(Error::invalid(
                        "a bearer credential has no companion header; a source that reads an \
                         identifier beside its token is declared as `header` with both named",
                    ));
                }
                if self.secret.is_none() {
                    return Err(Error::invalid(
                        "the auth scheme is `bearer` but no secret variable is named",
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Where the source lives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointSpec {
    /// `http://host[:port]`. `None` means unconfigured, which is what makes a
    /// connector report itself unavailable instead of guessing an address.
    ///
    /// Plaintext by name: `qip_transport::http` has no TLS stack and refuses
    /// `https` rather than downgrading it, so a public HTTPS source is reached
    /// through a TLS-terminating egress proxy whose address goes here. See
    /// `docs/data-sources/connector-development.md`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Path under `base_url`. May not carry a query: the query is built from
    /// [`Self::query`] plus whatever the cursor adds, and a second `?` puts
    /// the parameters where the source does not read them.
    pub path: String,
    /// Fixed query parameters, in a deterministic order.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub query: BTreeMap<String, String>,
    /// A cheap path that proves the source is alive without fetching data.
    /// Defaults to [`Self::path`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_path: Option<String>,
}

impl EndpointSpec {
    pub fn health_path(&self) -> &str {
        self.health_path.as_deref().unwrap_or(&self.path)
    }

    pub fn validate(&self) -> Result<()> {
        for (label, path) in [("path", &self.path), ("health_path", &self.path)] {
            if path.trim().is_empty() {
                return Err(Error::invalid(format!("the endpoint `{label}` is empty")));
            }
            if path.contains('?') || path.contains('#') {
                return Err(Error::invalid(format!(
                    "the endpoint `{label}` {path:?} carries a query or a fragment; the query is \
                     built from the manifest's `query` and the cursor, and a second one would \
                     put the parameters where the source does not read them"
                )));
            }
        }
        for (key, value) in &self.query {
            if key.trim().is_empty() {
                return Err(Error::invalid("a query parameter with an empty name"));
            }
            for text in [key.as_str(), value.as_str()] {
                if text
                    .chars()
                    .any(|c| c.is_control() || c == ' ' || c == '&' || c == '#')
                {
                    return Err(Error::invalid(format!(
                        "the query parameter {key}={value} contains a character that would split \
                         the request line or start another parameter"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// What the source will tolerate being asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RateLimitSpec {
    /// Sustained requests permitted per [`Self::per_ms`].
    pub requests: u32,
    pub per_ms: i64,
    /// How many requests may be spent at once. Never below `requests`.
    pub burst: u32,
}

impl RateLimitSpec {
    pub const fn per(&self) -> Duration {
        Duration::from_millis(self.per_ms)
    }

    /// The shortest interval between two requests that stays inside the
    /// sustained rate.
    pub fn min_interval(&self) -> Duration {
        if self.requests == 0 {
            return Duration::from_millis(self.per_ms);
        }
        Duration::from_nanos(self.per().as_nanos() / i64::from(self.requests))
    }

    pub fn validate(&self) -> Result<()> {
        if self.requests == 0 {
            return Err(Error::invalid(
                "a rate limit of zero requests never fetches anything",
            ));
        }
        if self.per_ms <= 0 {
            return Err(Error::invalid(
                "a rate-limit window must be a positive span, or the rate is undefined",
            ));
        }
        if self.burst < self.requests {
            return Err(Error::invalid(format!(
                "the burst {} is below the sustained rate {}, so the limiter would refuse \
                 requests the source permits",
                self.burst, self.requests
            )));
        }
        Ok(())
    }
}

/// How hard to try again after a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetrySpec {
    /// Total attempts including the first. `1` means no retry.
    pub max_attempts: u32,
    pub initial_backoff_ms: i64,
    pub max_backoff_ms: i64,
    pub multiplier: u32,
    /// How far *below* the exponential value the jitter may pull, in basis
    /// points. Downward, so the maximum stays a real maximum.
    pub jitter_basis_points: u32,
}

impl RetrySpec {
    /// The manifest's retry stanza as the transport's own policy.
    ///
    /// Built on `qip_transport::RetryPolicy` rather than beside it: two
    /// backoff implementations in one process are two ladders that disagree
    /// during the outage they both exist for.
    pub const fn policy(&self) -> RetryPolicy {
        RetryPolicy {
            max_attempts: self.max_attempts,
            initial_backoff: Duration::from_millis(self.initial_backoff_ms),
            max_backoff: Duration::from_millis(self.max_backoff_ms),
            multiplier: self.multiplier,
            jitter_basis_points: self.jitter_basis_points,
        }
    }

    pub fn validate(&self) -> Result<()> {
        self.policy().validate()
    }
}

impl Default for RetrySpec {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            initial_backoff_ms: 200,
            max_backoff_ms: 5_000,
            multiplier: 3,
            jitter_basis_points: 2_500,
        }
    }
}

/// One data source, entirely as configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceManifest {
    /// Stable identity. Recorded as the provenance source of every record,
    /// so changing it renames the origin of everything already published.
    pub source_id: String,
    pub provider: String,
    pub asset_class: AssetClass,
    pub region: Region,
    pub protocol: Protocol,
    pub schema: SchemaContract,
    #[serde(default)]
    pub auth: AuthSpec,
    pub endpoint: EndpointSpec,
    pub rate_limit: RateLimitSpec,
    #[serde(default)]
    pub retry: RetrySpec,
    /// How often the runtime intends to fetch. Never below the rate limit's
    /// minimum interval.
    pub poll_interval_ms: i64,
    /// How old the newest event may be before the feed is stale. The number
    /// an alert fires on.
    pub freshness_sla_ms: i64,
    /// How long after an event the source publishes it. Decides when a record
    /// became knowable, which is not when it occurred.
    #[serde(default)]
    pub publication_delay_ms: i64,
    pub licensing: LicensingClass,
    /// Most events one response may yield, so a well-formed large answer
    /// cannot become an unbounded allocation downstream.
    pub max_events_per_batch: usize,
}

impl SourceManifest {
    /// Parse and validate a manifest.
    ///
    /// Validation is not optional and there is no constructor that skips it: a
    /// manifest whose poll interval breaches its own rate limit is a ban, and
    /// the only place to catch it is before the first request.
    pub fn from_json(text: &str) -> Result<Self> {
        let manifest: Self = serde_json::from_str(text)
            .map_err(|error| Error::schema(format!("this is not a source manifest: {error}")))?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self)
            .map_err(|error| Error::schema(format!("the manifest could not be written: {error}")))
    }

    pub const fn poll_interval(&self) -> Duration {
        Duration::from_millis(self.poll_interval_ms)
    }

    pub const fn freshness_sla(&self) -> Duration {
        Duration::from_millis(self.freshness_sla_ms)
    }

    pub const fn publication_delay(&self) -> Duration {
        Duration::from_millis(self.publication_delay_ms)
    }

    /// Whether the source can be reached at all with what is configured.
    pub fn is_configured(&self) -> bool {
        self.missing_configuration().is_empty()
    }

    /// What the deployment has not supplied, each named on its own so an
    /// operator with two of three learns which one is left.
    pub fn missing_configuration(&self) -> Vec<String> {
        self.missing_configuration_with(&SecretRef::resolve)
    }

    /// [`Self::missing_configuration`] against a resolver the caller supplies.
    ///
    /// Every secret the manifest names is checked — the primary and, where
    /// there is one, the companion — each reported on its own line. A
    /// deployment that mounted Alpaca's secret key and forgot the key id
    /// would otherwise pass this check and answer 401 at the health probe,
    /// which names neither variable.
    pub fn missing_configuration_with(&self, resolve: SecretResolver<'_>) -> Vec<String> {
        let mut missing = Vec::new();
        if self.endpoint.base_url.is_none() {
            missing.push(format!(
                "no endpoint: set `endpoint.base_url` to the plaintext address this connector \
                 requests `{}` under. `qip_transport::http` has no TLS stack and refuses \
                 `https` by name, so a public HTTPS source is reached through a \
                 TLS-terminating egress proxy",
                self.endpoint.path
            ));
        }
        for secret in self.auth.secrets() {
            let present = matches!(resolve(secret), Ok(Some(_)));
            if !present {
                missing.push(format!(
                    "no credential: the deployment must set `{}` (or `{}_FILE`), which this \
                     manifest names and never contains",
                    secret.variable(),
                    secret.variable()
                ));
            }
        }
        missing
    }

    pub fn validate(&self) -> Result<()> {
        if self.source_id.trim().is_empty() {
            return Err(Error::invalid(
                "a manifest needs a source id: it is the provenance source of every record the \
                 connector produces",
            ));
        }
        if !self
            .source_id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(Error::invalid(format!(
                "the source id {:?} must be lower-case ASCII with digits and hyphens: it becomes \
                 a metric label, a topic key and a directory name, and each of those rejects a \
                 different character",
                self.source_id
            )));
        }
        if self.provider.trim().is_empty() {
            return Err(Error::invalid(format!(
                "source `{}` must name its provider; an unattributed source cannot be asked for \
                 terms and cannot be held to any",
                self.source_id
            )));
        }
        self.auth.validate()?;
        self.endpoint.validate()?;
        self.rate_limit.validate()?;
        self.retry.validate()?;
        if self.max_events_per_batch == 0 {
            return Err(Error::invalid(
                "max_events_per_batch is zero, which would refuse every response the source sends",
            ));
        }
        if self.poll_interval_ms <= 0 {
            return Err(Error::invalid(
                "a poll interval must be a positive span; zero is a request loop with no pause",
            ));
        }
        if self.freshness_sla_ms <= 0 {
            return Err(Error::invalid(
                "a freshness SLA must be a positive span, or nothing is ever stale and the alarm \
                 never fires",
            ));
        }
        if self.publication_delay_ms < 0 {
            return Err(Error::invalid(
                "a negative publication delay would make records knowable before they occurred",
            ));
        }
        let floor = self.rate_limit.min_interval();
        if self.poll_interval() < floor {
            return Err(Error::invalid(format!(
                "source `{}` polls every {:?} and its own rate limit permits one request per \
                 {floor:?}; a manifest that breaches its own limit is a ban discovered in \
                 production",
                self.source_id,
                self.poll_interval()
            )));
        }
        if self.freshness_sla() < self.poll_interval() {
            return Err(Error::invalid(format!(
                "source `{}` has a freshness SLA of {:?} and polls every {:?}: the feed would be \
                 stale between every pair of polls, so the alarm would fire on the schedule \
                 rather than on the source",
                self.source_id,
                self.freshness_sla(),
                self.poll_interval()
            )));
        }
        Ok(())
    }
}
