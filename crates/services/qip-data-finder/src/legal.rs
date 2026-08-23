//! Whether the platform may collect a source at all.
//!
//! Everything here exists to make one property structural rather than
//! advisory: **unknown is not permitted**. [`Legality`] has three values, not
//! two, and the only way to combine two of them is [`Legality::and`], which
//! takes the least permissive. A caller cannot accidentally read an
//! undetermined licence as a grant, because there is no boolean to read it
//! into — `is_permitted` answers `false` for `Unknown` and the reason travels
//! with the value.
//!
//! The alternative — a `bool` plus a comment — fails in a specific, familiar
//! way: the robots fetch times out, the licence page 404s, and a source with
//! no stated terms becomes indistinguishable from one whose terms permit
//! everything. That is exactly the source that ends up in a regulator's letter.

use qip_contracts::governance::{Entitlement, Usage};
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A three-valued verdict on whether something is allowed.
///
/// Ordered by permissiveness: `Forbidden` < `Unknown` < `Permitted`. Only
/// `Permitted` permits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "legality", rename_all = "snake_case")]
pub enum Legality {
    /// Allowed, on a stated basis.
    Permitted { basis: String },
    /// Not allowed, by a stated rule.
    Forbidden { rule: String },
    /// Could not be determined. Treated as not allowed everywhere in this
    /// crate, and carrying the question that has to be answered to move it.
    Unknown { question: String },
}

impl Legality {
    pub fn permitted(basis: impl Into<String>) -> Self {
        Self::Permitted {
            basis: basis.into(),
        }
    }

    pub fn forbidden(rule: impl Into<String>) -> Self {
        Self::Forbidden { rule: rule.into() }
    }

    pub fn unknown(question: impl Into<String>) -> Self {
        Self::Unknown {
            question: question.into(),
        }
    }

    /// Whether this verdict allows collection. `Unknown` does not.
    pub fn is_permitted(&self) -> bool {
        matches!(self, Self::Permitted { .. })
    }

    pub fn is_forbidden(&self) -> bool {
        matches!(self, Self::Forbidden { .. })
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown { .. })
    }

    /// How permissive this verdict is. Lower is stricter.
    pub const fn permissiveness(&self) -> u8 {
        match self {
            Self::Forbidden { .. } => 0,
            Self::Unknown { .. } => 1,
            Self::Permitted { .. } => 2,
        }
    }

    /// Combine two verdicts, keeping the least permissive.
    ///
    /// This is the only combinator, and it has no permissive counterpart on
    /// purpose. An `or` would let a source that is forbidden by robots and
    /// licensed by contract come out permitted, and the two questions are not
    /// alternatives — both have to be answered the same way.
    pub fn and(self, other: Self) -> Self {
        if other.permissiveness() < self.permissiveness() {
            other
        } else {
            self
        }
    }

    /// The reason, whichever variant this is.
    pub fn reason(&self) -> &str {
        match self {
            Self::Permitted { basis } => basis,
            Self::Forbidden { rule } => rule,
            Self::Unknown { question } => question,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Permitted { basis } => format!("permitted: {basis}"),
            Self::Forbidden { rule } => format!("forbidden: {rule}"),
            Self::Unknown { question } => format!("undetermined: {question}"),
        }
    }

    /// Refuse anything that is not explicitly permitted.
    pub fn require_permitted(&self, subject: &str) -> Result<()> {
        match self {
            Self::Permitted { .. } => Ok(()),
            Self::Forbidden { rule } => Err(Error::denied(format!(
                "{subject} may not be collected: {rule}"
            ))),
            Self::Unknown { question } => Err(Error::denied(format!(
                "{subject} may not be collected because its legality is undetermined: \
                 {question}. Absence of a prohibition is not a permission"
            ))),
        }
    }
}

/// A licence the platform has actually read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLicense {
    identifier: String,
    permits: BTreeSet<Usage>,
    attribution_required: bool,
    expires_at: Option<Timestamp>,
}

impl SourceLicense {
    /// The expiry recorded for a licence that does not expire.
    ///
    /// Deliberately not [`Timestamp::MAX`]. The platform serialises
    /// timestamps at millisecond precision, so a nanosecond sentinel comes
    /// back a fraction earlier and a decision read out of the evidence store
    /// no longer equals the one that was written into it — which would make
    /// the evidence store's whole point, that a record is the record, false
    /// for exactly the entitlements that never lapse. Floored to the
    /// millisecond it round-trips exactly and means the same thing.
    pub const PERPETUAL: Timestamp = Timestamp::from_millis(i64::MAX / 1_000_000);

    /// Build a licence from its identifier and the usages it grants.
    ///
    /// An unnamed licence is refused: "we believe this is fine" is the claim
    /// this type exists to make unrepresentable.
    pub fn new(
        identifier: impl Into<String>,
        permits: impl IntoIterator<Item = Usage>,
    ) -> Result<Self> {
        let identifier = identifier.into();
        if identifier.trim().is_empty() {
            return Err(Error::invalid(
                "a licence must be identified; an unnamed licence is an assumption",
            ));
        }
        Ok(Self {
            identifier,
            permits: permits.into_iter().collect(),
            attribution_required: false,
            expires_at: None,
        })
    }

    pub fn requiring_attribution(mut self) -> Self {
        self.attribution_required = true;
        self
    }

    pub fn expiring_at(mut self, at: Timestamp) -> Self {
        self.expires_at = Some(at);
        self
    }

    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    pub fn permits(&self) -> &BTreeSet<Usage> {
        &self.permits
    }

    pub fn attribution_required(&self) -> bool {
        self.attribution_required
    }

    pub fn expires_at(&self) -> Option<Timestamp> {
        self.expires_at
    }

    /// Whether the licence grants `usage` and has not lapsed.
    pub fn permits_at(&self, usage: Usage, now: Timestamp) -> bool {
        if let Some(expiry) = self.expires_at
            && now >= expiry
        {
            return false;
        }
        self.permits.contains(&usage)
    }

    /// The licence rendered as the entitlements `qip-compliance` enforces.
    ///
    /// Every usage gets an entry — granted or explicitly denied — so a
    /// downstream check reads a decision rather than an absence. The same
    /// [`Entitlement`] type the mesh catalogue holds, so the two cannot
    /// disagree about what a licence says.
    pub fn entitlements(&self, dataset: &str, now: Timestamp) -> Vec<Entitlement> {
        let expires_at = self.expires_at.unwrap_or(Self::PERPETUAL);
        [
            Usage::Research,
            Usage::Derive,
            Usage::Trade,
            Usage::Redistribute,
        ]
        .into_iter()
        .map(|usage| {
            if self.permits_at(usage, now) {
                Entitlement::Granted {
                    dataset: dataset.to_string(),
                    usage,
                    expires_at,
                }
            } else {
                Entitlement::Denied {
                    dataset: dataset.to_string(),
                    usage,
                    reason: format!(
                        "licence `{}` does not grant {}",
                        self.identifier,
                        usage.as_str()
                    ),
                }
            }
        })
        .collect()
    }
}

/// What is known about a source's licensing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "posture", rename_all = "snake_case")]
pub enum LicensingPosture {
    /// Terms were found, read, and mapped onto usages.
    Declared { license: SourceLicense },
    /// Terms exist and could not be mapped onto usages. Distinct from
    /// [`Self::Undetermined`] because the remedy differs: this one needs a
    /// lawyer, that one needs a fetch.
    Ambiguous { evidence: String },
    /// No terms were found at all.
    Undetermined,
}

impl LicensingPosture {
    pub fn declared(license: SourceLicense) -> Self {
        Self::Declared { license }
    }

    pub fn ambiguous(evidence: impl Into<String>) -> Self {
        Self::Ambiguous {
            evidence: evidence.into(),
        }
    }

    pub fn license(&self) -> Option<&SourceLicense> {
        match self {
            Self::Declared { license } => Some(license),
            Self::Ambiguous { .. } | Self::Undetermined => None,
        }
    }

    /// Whether this posture permits `usage` at `now`.
    ///
    /// A source licensed for research and asked about trading comes back
    /// `Forbidden`, not `Unknown`: the terms were read and they do not grant
    /// it. That difference is what stops a research feed being promoted onto
    /// the trading path by anyone who only checks for the absence of a
    /// prohibition.
    pub fn legality_for(&self, usage: Usage, now: Timestamp) -> Legality {
        match self {
            Self::Declared { license } => {
                if license.permits_at(usage, now) {
                    Legality::permitted(format!(
                        "licence `{}` grants {}",
                        license.identifier(),
                        usage.as_str()
                    ))
                } else if license.expires_at().is_some_and(|expiry| now >= expiry) {
                    Legality::forbidden(format!(
                        "licence `{}` expired before {now}",
                        license.identifier()
                    ))
                } else {
                    Legality::forbidden(format!(
                        "licence `{}` grants {} and not {}",
                        license.identifier(),
                        license
                            .permits()
                            .iter()
                            .map(|granted| granted.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                        usage.as_str()
                    ))
                }
            }
            Self::Ambiguous { evidence } => Legality::unknown(format!(
                "the terms were found but not mapped onto a usage ({evidence}); \
                 which usages `{}` covers has to be decided by a human",
                usage.as_str()
            )),
            Self::Undetermined => Legality::unknown(
                "no licence or terms of use were located for this source".to_string(),
            ),
        }
    }
}

/// Hosts the platform may and may not reach.
///
/// Deny is evaluated first and unconditionally. The two lists are not
/// symmetric: an allowlist entry is a convenience, and a denylist entry is
/// normally a legal instruction, an incident, or a publisher who asked. A
/// host on both is a configuration mistake, and the safe reading of a mistake
/// is the restrictive one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRules {
    allowlist: BTreeSet<String>,
    denylist: BTreeSet<String>,
}

impl HostRules {
    /// No restriction beyond what each source's own terms say.
    pub fn open() -> Self {
        Self::default()
    }

    pub fn new(
        allowlist: impl IntoIterator<Item = String>,
        denylist: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            allowlist: allowlist
                .into_iter()
                .map(|host| host.to_ascii_lowercase())
                .collect(),
            denylist: denylist
                .into_iter()
                .map(|host| host.to_ascii_lowercase())
                .collect(),
        }
    }

    pub fn allowlist(&self) -> &BTreeSet<String> {
        &self.allowlist
    }

    pub fn denylist(&self) -> &BTreeSet<String> {
        &self.denylist
    }

    /// Whether this host may be contacted at all.
    ///
    /// Matching covers subdomains: denying `example.com` denies
    /// `api.example.com`, because a publisher who asked us to stop did not
    /// mean "stop on this hostname only".
    pub fn verdict(&self, host: &str) -> Legality {
        let host = host.to_ascii_lowercase();
        if let Some(entry) = self.denylist.iter().find(|entry| covers(entry, &host)) {
            return Legality::forbidden(format!(
                "host `{host}` is denylisted by `{entry}`; a denylisted host is unreachable \
                 however it scores and whatever else permits it"
            ));
        }
        if self.allowlist.is_empty() {
            return Legality::permitted(
                "no host allowlist is configured, so no host is excluded by one".to_string(),
            );
        }
        match self.allowlist.iter().find(|entry| covers(entry, &host)) {
            Some(entry) => {
                Legality::permitted(format!("host `{host}` is allowlisted by `{entry}`"))
            }
            None => Legality::forbidden(format!(
                "host `{host}` is not on the configured egress allowlist"
            )),
        }
    }
}

/// Whether `entry` covers `host`, exactly or as a parent domain.
fn covers(entry: &str, host: &str) -> bool {
    host == entry || host.ends_with(&format!(".{entry}"))
}

/// A request budget, as a publisher states it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimit {
    requests: u32,
    per: Duration,
}

impl RateLimit {
    pub fn new(requests: u32, per: Duration) -> Result<Self> {
        if requests == 0 {
            return Err(Error::invalid(
                "a rate limit of zero requests is a prohibition, not a limit; \
                 express it as a denylist entry",
            ));
        }
        if per.as_nanos() <= 0 {
            return Err(Error::invalid("a rate limit must span a positive period"));
        }
        Ok(Self { requests, per })
    }

    pub fn requests(&self) -> u32 {
        self.requests
    }

    pub fn per(&self) -> Duration {
        self.per
    }

    /// The shortest gap between requests that stays inside the budget.
    ///
    /// Spacing requests evenly rather than spending the budget in a burst and
    /// waiting: a burst is what a publisher's rate limiter sees as an attack,
    /// and being right about the average does not help once we are blocked.
    pub fn min_interval(&self) -> Duration {
        Duration::from_nanos(self.per.as_nanos() / i64::from(self.requests).max(1))
    }
}

/// The collection policy actually emitted for a source.
///
/// This is the artefact an adapter obeys. It exists as a value rather than as
/// advice in a document so that the crawl delay a publisher asked for is
/// carried by the same object that says the source may be collected.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourcePolicy {
    host: String,
    user_agent: String,
    crawl_delay: Option<Duration>,
    declared_rate_limit: Option<RateLimit>,
    min_request_interval: Duration,
    disallowed_paths: Vec<String>,
    collection_permitted: bool,
    attribution_required: bool,
}

impl SourcePolicy {
    /// The floor between requests when nothing states one.
    ///
    /// A publisher who says nothing has not consented to being hammered, so
    /// the default is a rate a human browsing would produce rather than the
    /// fastest the network allows.
    pub const DEFAULT_MIN_INTERVAL: Duration = Duration::from_secs(1);

    /// Assemble the policy from everything that constrains it.
    ///
    /// The interval is the maximum of the crawl delay and the rate limit,
    /// never a blend: both are ceilings, and obeying the looser one breaches
    /// the tighter one.
    pub fn assemble(
        host: impl Into<String>,
        user_agent: impl Into<String>,
        crawl_delay: Option<Duration>,
        declared_rate_limit: Option<RateLimit>,
        disallowed_paths: Vec<String>,
        collection_permitted: bool,
    ) -> Self {
        let from_rate = declared_rate_limit.map(|limit| limit.min_interval());
        let min_request_interval = [crawl_delay, from_rate, Some(Self::DEFAULT_MIN_INTERVAL)]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(Self::DEFAULT_MIN_INTERVAL);
        Self {
            host: host.into(),
            user_agent: user_agent.into(),
            crawl_delay,
            declared_rate_limit,
            min_request_interval,
            disallowed_paths,
            collection_permitted,
            attribution_required: false,
        }
    }

    pub fn requiring_attribution(mut self, required: bool) -> Self {
        self.attribution_required = required;
        self
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    /// The identity this crawler presents. A publisher's only recourse is to
    /// block a name, so the name has to be real and stable.
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    pub fn crawl_delay(&self) -> Option<Duration> {
        self.crawl_delay
    }

    pub fn declared_rate_limit(&self) -> Option<RateLimit> {
        self.declared_rate_limit
    }

    pub fn min_request_interval(&self) -> Duration {
        self.min_request_interval
    }

    pub fn disallowed_paths(&self) -> &[String] {
        &self.disallowed_paths
    }

    pub fn collection_permitted(&self) -> bool {
        self.collection_permitted
    }

    pub fn attribution_required(&self) -> bool {
        self.attribution_required
    }

    /// The earliest a next request may be made after one at `last`.
    pub fn earliest_next_request(&self, last: Timestamp) -> Timestamp {
        last.saturating_add(self.min_request_interval)
    }

    /// How many requests this policy permits over `window`.
    pub fn permitted_requests_over(&self, window: Duration) -> u64 {
        let interval = self.min_request_interval.as_nanos().max(1);
        (window.as_nanos().max(0) / interval).max(0) as u64
    }
}

/// Every legality question answered for one source, and the verdict they
/// combine to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LegalAssessment {
    host: Legality,
    robots: Legality,
    licensing: Legality,
    usage: Usage,
    overall: Legality,
}

impl LegalAssessment {
    /// Combine the three questions. The overall verdict is the least
    /// permissive of them, computed here rather than by the caller so there
    /// is one place where the combination can be got wrong.
    pub fn combine(host: Legality, robots: Legality, licensing: Legality, usage: Usage) -> Self {
        let overall = host.clone().and(robots.clone()).and(licensing.clone());
        Self {
            host,
            robots,
            licensing,
            usage,
            overall,
        }
    }

    pub fn host(&self) -> &Legality {
        &self.host
    }

    pub fn robots(&self) -> &Legality {
        &self.robots
    }

    pub fn licensing(&self) -> &Legality {
        &self.licensing
    }

    /// The usage the licensing question was asked about.
    pub fn usage(&self) -> Usage {
        self.usage
    }

    pub fn overall(&self) -> &Legality {
        &self.overall
    }

    pub fn is_collectible(&self) -> bool {
        self.overall.is_permitted()
    }

    /// Each question and its answer, for the decision record.
    pub fn findings(&self) -> BTreeMap<&'static str, String> {
        BTreeMap::from([
            ("host", self.host.describe()),
            ("robots", self.robots.describe()),
            ("licensing", self.licensing.describe()),
        ])
    }
}
