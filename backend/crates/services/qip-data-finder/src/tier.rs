//! The three tiers of the web a source can sit on, and what each one permits.
//!
//! The blueprint (§7.4–§7.6) splits discovery into a surface web that search
//! engines index, a deep web that they do not — query forms, rendered pages,
//! registrations and licensed subscriptions — and a dark web of hidden
//! services that is watched defensively and never read for signal. The
//! distinction is legal before it is technical: a deep-web source is lawful
//! when its access is clean, and a dark-web source is never a trading input
//! however it was reached. This module makes both of those typed policy
//! rather than a paragraph somebody has to remember.
//!
//! Four things hold here by construction:
//!
//! * **A tier is classified from evidence, never assumed.**
//!   [`SourceTier::classify`] refuses on insufficient evidence instead of
//!   defaulting to `SurfaceWeb`. The failure this prevents is a candidate that
//!   nobody has probed being routed as an open API because "surface" was the
//!   cheapest answer, and then turning out to be a login wall.
//! * **A credential is referenced by name, never carried.**
//!   [`CredentialReference::new`] refuses anything that does not look like a
//!   name the deployment resolves to a file. A value pasted here would be
//!   serialised into every decision record and every catalogue entry, which is
//!   the leak `.claude/rules/01-security-and-safety.md` exists to prevent.
//! * **Rendering and bulk extraction happen only inside a named enclave.**
//!   [`DeepWebAdapter::admissible`] refuses the `rendered` and `bulk` modes
//!   without a [`DiscoveryEnclave`], and the enclave carries no field in which
//!   a trading-zone credential could ever be placed.
//! * **The dark web has no fetch path.** [`DeepWebAdapter::admissible`]
//!   refuses every access mode for [`SourceTier::DarkWeb`] by name, and
//!   [`DefensiveMonitoring`] is a record of *what is watched* with no method
//!   that reaches a network — this crate opens no sockets, and nothing in it
//!   would know how to reach a hidden service if it did.
//!
//! Everything here is data and rules over data. The transport a deployment
//! would need — a headless renderer, a Tor client — is deliberately absent
//! (ADR 0009), so the enclave is a policy the router names, not a process
//! this crate starts.

use crate::endpoint::AccessMechanism;
use crate::legal::{LicensingPosture, RateLimit};
use crate::probe::RobotsFetch;
use crate::source::{Source, SourceCandidate};
use qip_core::Duration;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Where on the web a source lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceTier {
    /// Indexed by search engines and reachable without a credential.
    SurfaceWeb,
    /// Unindexed: behind a query form, a rendering step, a registration or a
    /// licence. Lawful when the access is clean, and where the blueprint says
    /// the informational edge lives.
    DeepWeb,
    /// Hidden services. Watched for the platform's own exposure and never
    /// read for signal.
    DarkWeb,
}

impl SourceTier {
    /// Host suffixes that identify a hidden service.
    ///
    /// Matched as whole labels, so `onion.example` is not dark and
    /// `market.onion` is.
    pub const DARK_HOST_SUFFIXES: [&'static str; 2] = ["onion", "i2p"];

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::SurfaceWeb => "surface_web",
            Self::DeepWeb => "deep_web",
            Self::DarkWeb => "dark_web",
        }
    }

    /// Whether facts from this tier may reach the world model and training.
    ///
    /// The blueprint's rule is "deep web trains; dark web defends", and the
    /// two are never merged.
    pub const fn feeds_training(&self) -> bool {
        !matches!(self, Self::DarkWeb)
    }

    /// Whether a host names a hidden service.
    pub fn is_dark_host(host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        host.rsplit('.')
            .next()
            .is_some_and(|last| Self::DARK_HOST_SUFFIXES.contains(&last))
    }

    /// Classify a source from what is actually known about it.
    ///
    /// The rules, in order, each on evidence the finder holds rather than on
    /// a claim:
    ///
    /// 1. A hidden-service host is `DarkWeb`, whatever else is true. This is
    ///    decidable before any request is made, and it has to be, because the
    ///    one thing a dark-web candidate must never receive is a probe.
    /// 2. An endpoint that declares a credential requirement is `DeepWeb`:
    ///    it is login-gated by its own description.
    /// 3. A page read by extraction is `DeepWeb`. This crate has no renderer,
    ///    so an extracted page is reached by the blueprint's *rendered* mode,
    ///    which it places in the deep tier under the enclave's governance.
    /// 4. An unauthenticated, machine-readable endpoint is `SurfaceWeb` when
    ///    a probe has seen it answer without a credential and its robots
    ///    posture has been established, and `DeepWeb` when the probe was
    ///    turned away for want of one — the candidate's claim of open access
    ///    was wrong and the source is gated in fact.
    ///
    /// Anything short of that is refused with the missing evidence named.
    /// There is no default arm, because a default tier is a guess that every
    /// later routing decision would inherit as if it were a finding.
    pub fn classify(evidence: &TierEvidence) -> Result<Self> {
        if Self::is_dark_host(&evidence.host) {
            return Ok(Self::DarkWeb);
        }
        if evidence.credential_required.is_some() {
            return Ok(Self::DeepWeb);
        }
        if evidence.interface == Interface::ExtractedPage {
            return Ok(Self::DeepWeb);
        }
        if evidence.robots == RobotsPosture::NotFetched {
            return Err(Error::invalid(format!(
                "`{}` cannot be placed in a tier yet: its robots posture has not been \
                 established, and an unauthenticated endpoint is surface web only once a \
                 probe has seen how its origin governs crawlers. Probe it first",
                evidence.host
            )));
        }
        match evidence.unauthenticated_reachability {
            Some(true) => Ok(Self::SurfaceWeb),
            Some(false) => Ok(Self::DeepWeb),
            None => Err(Error::invalid(format!(
                "`{}` cannot be placed in a tier: no probe has shown whether it answers \
                 without a credential, and surface web is a finding about reachability, \
                 not the absence of a declared login. Probe it first",
                evidence.host
            ))),
        }
    }
}

/// Whether a source is read as data or as a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interface {
    /// An API, feed, file, repository, socket or multicast group.
    MachineReadable,
    /// A page with no machine interface, read by extraction.
    ExtractedPage,
}

impl Interface {
    pub fn of(mechanism: &AccessMechanism) -> Self {
        match mechanism {
            AccessMechanism::HtmlPage { .. } => Self::ExtractedPage,
            AccessMechanism::Rest { .. }
            | AccessMechanism::WebSocket { .. }
            | AccessMechanism::Feed { .. }
            | AccessMechanism::BulkFile { .. }
            | AccessMechanism::GitRepository { .. }
            | AccessMechanism::Mcp { .. }
            | AccessMechanism::StreamingMulticast { .. } => Self::MachineReadable,
        }
    }
}

/// What is known about a host's robots.txt at the moment of classification.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RobotsPosture {
    /// The mechanism is not one robots.txt speaks to; its terms are a licence.
    NotGoverned,
    /// Nothing has been asked yet. Distinct from every answer.
    NotFetched,
    Served,
    Absent,
    Unreachable,
}

impl RobotsPosture {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::NotGoverned => "not_governed",
            Self::NotFetched => "not_fetched",
            Self::Served => "served",
            Self::Absent => "absent",
            Self::Unreachable => "unreachable",
        }
    }
}

/// The evidence a tier is classified from.
///
/// Built from a [`SourceCandidate`] before a probe and from a [`Source`]
/// after one, and the difference between the two is exactly the evidence a
/// probe supplies: the robots posture and whether the endpoint answered
/// without a credential. Fields the finder does not yet know are `None` or
/// `NotFetched`, never a plausible value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierEvidence {
    host: String,
    interface: Interface,
    /// What the endpoint says a deployment must supply, in the endpoint's
    /// own words. `None` means it declares no credential.
    credential_required: Option<String>,
    robots: RobotsPosture,
    /// `Some(true)` when a probe's HEAD succeeded with no credential,
    /// `Some(false)` when it was refused for want of one, `None` when no
    /// probe has answered the question.
    unauthenticated_reachability: Option<bool>,
}

impl TierEvidence {
    /// Everything a candidate's own description says, and nothing observed.
    pub fn from_candidate(candidate: &SourceCandidate) -> Self {
        let mechanism = candidate.endpoint().mechanism();
        Self {
            host: candidate.endpoint().host().to_string(),
            interface: Interface::of(mechanism),
            credential_required: mechanism.poll_plan().credential_required,
            robots: if mechanism.is_governed_by_robots() {
                RobotsPosture::NotFetched
            } else {
                RobotsPosture::NotGoverned
            },
            unauthenticated_reachability: None,
        }
    }

    /// The candidate's description plus what the probe saw.
    pub fn from_source(source: &Source) -> Self {
        let mut evidence = Self::from_candidate(source.candidate());
        if source.endpoint().mechanism().is_governed_by_robots() {
            evidence.robots = match source.evidence().robots() {
                RobotsFetch::Served { .. } => RobotsPosture::Served,
                RobotsFetch::Absent { .. } => RobotsPosture::Absent,
                RobotsFetch::Unreachable { .. } => RobotsPosture::Unreachable,
            };
        }
        // Only a probe made without a credential can say anything about
        // unauthenticated reachability, so the answer is recorded only for
        // endpoints that declared none.
        if evidence.credential_required.is_none() {
            evidence.unauthenticated_reachability = match source.evidence().head().status {
                200..=299 => Some(true),
                401 | 403 | 407 => Some(false),
                _ => None,
            };
        }
        evidence
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn interface(&self) -> Interface {
        self.interface
    }

    pub fn credential_required(&self) -> Option<&str> {
        self.credential_required.as_deref()
    }

    pub fn robots(&self) -> RobotsPosture {
        self.robots
    }

    pub fn unauthenticated_reachability(&self) -> Option<bool> {
        self.unauthenticated_reachability
    }
}

/// A credential, by the name a deployment resolves to a file. Never a value.
///
/// The Secret Manager CSI driver projects each secret as a file, and
/// `qip_core::secret` reads it through the `_FILE` indirection. This type
/// holds only the name of that projection, so the value has no path into a
/// decision record, a catalogue entry or a log line.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CredentialReference {
    name: String,
}

impl CredentialReference {
    /// Longest name accepted. Secret Manager itself allows 255, but no
    /// credential *name* in this platform approaches that, and a token does.
    pub const MAX_LENGTH: usize = 64;

    /// Name a credential.
    ///
    /// Refuses a blank, an over-long string, or anything outside
    /// `[a-z0-9_-]` — which is what a name looks like and what an API key,
    /// a bearer token or a PEM block does not. Refusing on shape rather than
    /// on a `is_secret` heuristic means there is no value this accepts by
    /// looking harmless.
    pub fn new(name: impl Into<String>) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid(
                "a credential reference must name the secret the deployment projects as a \
                 file; an unnamed credential cannot be provided",
            ));
        }
        if name.len() > Self::MAX_LENGTH {
            return Err(Error::invalid(format!(
                "a credential reference is a name of at most {} characters, and this one is \
                 {}; a credential reference names the secret, it never carries the value",
                Self::MAX_LENGTH,
                name.len()
            )));
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        {
            return Err(Error::invalid(
                "a credential reference is a name in `[a-z0-9_-]`, resolved by the \
                 deployment to a projected file; it never carries the value itself",
            ));
        }
        Ok(Self { name })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

/// How much rendering a source may cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderingBudget {
    pages_per_hour: u32,
    max_render_time: Duration,
}

impl RenderingBudget {
    /// A renderer with no page budget is a crawler with no rate limit, and a
    /// renderer with no time budget is one a page can hold open forever.
    pub fn new(pages_per_hour: u32, max_render_time: Duration) -> Result<Self> {
        if pages_per_hour == 0 {
            return Err(Error::invalid(
                "a rendering budget of zero pages is a prohibition, not a budget",
            ));
        }
        if max_render_time.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a rendering budget must bound how long one page may take to render",
            ));
        }
        Ok(Self {
            pages_per_hour,
            max_render_time,
        })
    }

    pub fn pages_per_hour(&self) -> u32 {
        self.pages_per_hour
    }

    pub fn max_render_time(&self) -> Duration {
        self.max_render_time
    }
}

/// How often a bulk extract is fetched and how long it may be kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BulkCadence {
    every: Duration,
    retain_extract_for: Duration,
}

impl BulkCadence {
    /// The blueprint's bulk mode fetches an extract, takes the facts, and
    /// deletes the extract per the data policy. A cadence without a retention
    /// bound is an extract that is never deleted, which is the accumulation
    /// the pass-through rule forbids.
    pub fn new(every: Duration, retain_extract_for: Duration) -> Result<Self> {
        if every.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a bulk cadence must be a positive interval; a source that publishes extracts \
                 publishes them on a schedule",
            ));
        }
        if retain_extract_for.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a bulk extract must have a retention bound; an extract kept indefinitely is \
                 the hoarding pass-through forbids",
            ));
        }
        Ok(Self {
            every,
            retain_extract_for,
        })
    }

    pub fn every(&self) -> Duration {
        self.every
    }

    pub fn retain_extract_for(&self) -> Duration {
        self.retain_extract_for
    }
}

/// The six ways a deep-web source is reached, each carrying what it needs.
///
/// The blueprint's §7.6.2 table, as a type. What a mode *needs* is a field,
/// so a mode that needs a credential cannot exist without naming one, and a
/// mode that needs a budget cannot exist without stating it. What a mode
/// needs *from the deployment* — a declared licence, an enclave — is checked
/// by [`DeepWebAdapter::admissible`], because those are facts about where the
/// adapter runs rather than about the source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AccessMode {
    /// Parameterised requests to a form or search interface with no login.
    /// Respects robots.txt — the legality gate already refuses otherwise —
    /// and a rate limit, so parameters are enumerated rather than hammered.
    OpenQuery { rate_limit: RateLimit },
    /// A published API. Always preferred over scraping.
    Api,
    /// A legitimate free account. One per source, the credential projected
    /// as a file under the name given, terms of service honoured.
    Registered { credential: CredentialReference },
    /// Paid access in the operator's name. Names the licence the posture
    /// must have declared, so the two cannot describe different agreements.
    Licensed {
        credential: CredentialReference,
        licence: String,
    },
    /// Content produced by client-side code, rendered headlessly inside the
    /// discovery enclave within the budget.
    Rendered { budget: RenderingBudget },
    /// Periodic full extracts, fetched into the enclave's research cache and
    /// deleted on the cadence's retention bound.
    Bulk { cadence: BulkCadence },
}

impl AccessMode {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::OpenQuery { .. } => "open_query",
            Self::Api => "api",
            Self::Registered { .. } => "registered",
            Self::Licensed { .. } => "licensed",
            Self::Rendered { .. } => "rendered",
            Self::Bulk { .. } => "bulk",
        }
    }

    /// Whether this mode runs only inside a [`DiscoveryEnclave`].
    ///
    /// Rendering executes a source's code and a bulk fetch stores a source's
    /// whole extract; both are things the blueprint confines to the enclave
    /// because either can be hostile in ways a JSON response cannot.
    pub const fn needs_enclave(&self) -> bool {
        matches!(self, Self::Rendered { .. } | Self::Bulk { .. })
    }

    /// Whether this mode is admissible only under a declared licence.
    pub const fn needs_licence(&self) -> bool {
        matches!(self, Self::Licensed { .. })
    }

    /// The credential this mode needs the deployment to project, if any.
    pub fn credential(&self) -> Option<&CredentialReference> {
        match self {
            Self::Registered { credential } | Self::Licensed { credential, .. } => Some(credential),
            Self::OpenQuery { .. } | Self::Api | Self::Rendered { .. } | Self::Bulk { .. } => None,
        }
    }
}

/// The isolation policy a rendered or bulk fetch runs under.
///
/// A record of the boundary, not the boundary itself — this crate starts no
/// process. What it makes structural is the shape: there is no field here
/// in which a trading-zone credential could be placed, and egress is a set of
/// hosts that starts empty and admits one source at a time. The blueprint's
/// rule is "no path to any capital-moving component", and a policy type with
/// nowhere to put such a path is how that is held rather than promised.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryEnclave {
    name: String,
    /// Hosts a fetch may leave towards. Nothing else, including the
    /// platform's own services.
    egress_hosts: BTreeSet<String>,
    /// How long one fetch may run before it is killed.
    max_runtime: Duration,
}

impl DiscoveryEnclave {
    /// Name an enclave with a runtime bound.
    ///
    /// An unbounded runtime is a renderer a page can hold open forever, and
    /// an unnamed enclave is one no audit record can say a fetch ran in.
    pub fn new(name: impl Into<String>, max_runtime: Duration) -> Result<Self> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(Error::invalid(
                "a discovery enclave must be named, so a decision can say where a fetch ran",
            ));
        }
        if max_runtime.as_nanos() <= 0 {
            return Err(Error::invalid(
                "a discovery enclave must bound how long a fetch may run",
            ));
        }
        Ok(Self {
            name,
            egress_hosts: BTreeSet::new(),
            max_runtime,
        })
    }

    /// Permit egress to one source host.
    pub fn admitting_egress_to(mut self, host: impl Into<String>) -> Self {
        self.egress_hosts.insert(host.into().to_ascii_lowercase());
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn egress_hosts(&self) -> &BTreeSet<String> {
        &self.egress_hosts
    }

    pub fn max_runtime(&self) -> Duration {
        self.max_runtime
    }

    /// Whether a fetch inside this enclave may reach `host`.
    ///
    /// Exact match only. An enclave that admitted `example.com` and thereby
    /// `api.example.com` would be admitting a host nobody named.
    pub fn permits_egress_to(&self, host: &str) -> bool {
        self.egress_hosts.contains(&host.to_ascii_lowercase())
    }

    /// The enclave holds no trading-zone credential, and this is the only
    /// answer the type can give: there is no field to hold one.
    pub const fn holds_trading_zone_credential(&self) -> bool {
        false
    }

    pub fn describe(&self) -> String {
        format!(
            "enclave `{}`: egress to {} host(s) only, no trading-zone credential, runtime \
             bounded at {:?}",
            self.name,
            self.egress_hosts.len(),
            self.max_runtime
        )
    }
}

/// A registered source, the tier it was placed in, and the mode it is
/// reached by.
///
/// The blueprint's `DeepWebAdapter` also carries a query plan, an extractor
/// and entity links. Those are the ingestion adapter's business and live in
/// [`crate::ingestion`]; what lives here is the part that decides whether the
/// source may be reached at all, which has to be settled before any of the
/// rest is worth writing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepWebAdapter {
    source_id: String,
    host: String,
    tier: SourceTier,
    mode: AccessMode,
}

impl DeepWebAdapter {
    pub fn new(
        source_id: impl Into<String>,
        host: impl Into<String>,
        tier: SourceTier,
        mode: AccessMode,
    ) -> Result<Self> {
        let source_id = source_id.into();
        let host = host.into();
        if source_id.trim().is_empty() {
            return Err(Error::invalid(
                "an access adapter must name the source it reaches",
            ));
        }
        if host.trim().is_empty() {
            return Err(Error::invalid(format!(
                "the adapter for `{source_id}` must name the host it reaches, or an enclave \
                 cannot say whether egress to it is permitted"
            )));
        }
        Ok(Self {
            source_id,
            host,
            tier,
            mode,
        })
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn tier(&self) -> SourceTier {
        self.tier
    }

    pub fn mode(&self) -> &AccessMode {
        &self.mode
    }

    /// Whether this adapter may run, given what the deployment provides.
    ///
    /// The rules, each a refusal naming what would change the answer:
    ///
    /// * The dark web admits **no** mode. Every arm is refused by name, so a
    ///   new mode added to [`AccessMode`] is refused here without anyone
    ///   remembering to add it.
    /// * `licensed` needs the posture to have *declared* a licence, and the
    ///   same licence the mode names. An undetermined or ambiguous posture is
    ///   a subscription nobody has read the terms of.
    /// * `registered` needs its named credential, which the type already
    ///   guarantees; the refusal it can still produce is the tier's.
    /// * `rendered` and `bulk` need an enclave, and one whose egress admits
    ///   this host. An enclave that cannot reach the source is not the
    ///   enclave this fetch would run in.
    pub fn admissible(
        &self,
        posture: &LicensingPosture,
        enclave: Option<&DiscoveryEnclave>,
    ) -> Result<()> {
        if self.tier == SourceTier::DarkWeb {
            // Each arm is spelled out rather than folded into `_` so that the
            // refusal names the mode it refuses, and so that a seventh mode
            // cannot be added without a compile error landing here.
            let mode = match &self.mode {
                AccessMode::OpenQuery { .. } => "open_query",
                AccessMode::Api => "api",
                AccessMode::Registered { .. } => "registered",
                AccessMode::Licensed { .. } => "licensed",
                AccessMode::Rendered { .. } => "rendered",
                AccessMode::Bulk { .. } => "bulk",
            };
            return Err(Error::denied(format!(
                "`{}` on `{}` is on the dark web, and the dark web is monitoring-only: the \
                 `{mode}` access mode is refused, as every mode is. The platform watches \
                 hidden services for its own exposure through a `DefensiveMonitoring` record \
                 and never reads them for signal",
                self.source_id, self.host
            )));
        }
        match &self.mode {
            AccessMode::OpenQuery { .. } | AccessMode::Api | AccessMode::Registered { .. } => {
                Ok(())
            }
            AccessMode::Licensed { licence, .. } => match posture {
                LicensingPosture::Declared { license } if license.identifier() == licence => Ok(()),
                LicensingPosture::Declared { license } => Err(Error::denied(format!(
                    "`{}` is reached under licence `{licence}` but its posture declares `{}`; \
                     two claims about one subscription disagree, so neither is treated as \
                     current",
                    self.source_id,
                    license.identifier()
                ))),
                LicensingPosture::Ambiguous { .. } | LicensingPosture::Undetermined => {
                    Err(Error::denied(format!(
                        "`{}` is a licensed subscription and its licensing posture is not \
                         declared; a paid source is admissible only once its terms have been \
                         read and mapped onto usages",
                        self.source_id
                    )))
                }
            },
            AccessMode::Rendered { .. } | AccessMode::Bulk { .. } => {
                let Some(enclave) = enclave else {
                    return Err(Error::denied(format!(
                        "`{}` needs the `{}` access mode, which runs only inside a discovery \
                         enclave, and no enclave is configured; name one with \
                         `FinderConfig::with_discovery_enclave` and admit `{}` to its egress",
                        self.source_id,
                        self.mode.as_str(),
                        self.host
                    )));
                };
                if !enclave.permits_egress_to(&self.host) {
                    return Err(Error::denied(format!(
                        "`{}` needs the `{}` access mode inside enclave `{}`, whose egress does \
                         not admit `{}`; an enclave that cannot reach the source is not the \
                         enclave the fetch would run in",
                        self.source_id,
                        self.mode.as_str(),
                        enclave.name(),
                        self.host
                    )));
                }
                Ok(())
            }
        }
    }
}

/// What the platform watches the dark web for. Data, with no fetch path.
///
/// The blueprint's permitted column: the platform's own credentials or keys
/// appearing in a dump, a venue or custodian it uses being targeted, its
/// brand being impersonated. This record names those watch terms so a
/// monitoring feed — none exists in this build — would know what to match,
/// and it deliberately has no method that takes a probe, a host or a URL. A
/// type with no fetch method is how "never ingests content" is held.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefensiveMonitoring {
    /// The platform's own identifiers: service names, venue account labels,
    /// hostnames. Names, never values.
    own_identifiers: BTreeSet<String>,
    /// The *shapes* of credentials the platform issues — a prefix, a length,
    /// a format description — so a leak can be recognised without the
    /// watcher holding anything that could itself leak.
    credential_shapes: BTreeSet<String>,
    /// Brand terms whose appearance beside "for sale" or "breach" is a
    /// threat indicator.
    brand_terms: BTreeSet<String>,
}

impl DefensiveMonitoring {
    /// Build a watch list.
    ///
    /// Refuses an empty one, because a monitoring record that watches for
    /// nothing reads as monitoring and is not.
    pub fn new(
        own_identifiers: impl IntoIterator<Item = String>,
        credential_shapes: impl IntoIterator<Item = String>,
        brand_terms: impl IntoIterator<Item = String>,
    ) -> Result<Self> {
        let own_identifiers: BTreeSet<String> = own_identifiers.into_iter().collect();
        let credential_shapes: BTreeSet<String> = credential_shapes.into_iter().collect();
        let brand_terms: BTreeSet<String> = brand_terms.into_iter().collect();
        if own_identifiers.is_empty() && credential_shapes.is_empty() && brand_terms.is_empty() {
            return Err(Error::invalid(
                "a defensive monitoring record must watch for something; one that names no \
                 identifier, credential shape or brand term is monitoring in name only",
            ));
        }
        Ok(Self {
            own_identifiers,
            credential_shapes,
            brand_terms,
        })
    }

    pub fn own_identifiers(&self) -> &BTreeSet<String> {
        &self.own_identifiers
    }

    pub fn credential_shapes(&self) -> &BTreeSet<String> {
        &self.credential_shapes
    }

    pub fn brand_terms(&self) -> &BTreeSet<String> {
        &self.brand_terms
    }

    /// The tier this record is about. There is exactly one.
    pub const fn tier(&self) -> SourceTier {
        SourceTier::DarkWeb
    }

    pub fn describe(&self) -> String {
        format!(
            "defensive monitoring of the dark web: {} own identifier(s), {} credential \
             shape(s), {} brand term(s); registers indicators, never content",
            self.own_identifiers.len(),
            self.credential_shapes.len(),
            self.brand_terms.len()
        )
    }
}
