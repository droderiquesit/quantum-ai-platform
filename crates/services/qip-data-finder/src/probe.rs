//! The port through which the finder touches the outside world.
//!
//! Every network fact the lifecycle needs arrives through [`SourceProbe`]:
//! the robots.txt body, a HEAD of the candidate, a payload sample, and how
//! long each took. Nothing else in this crate opens a socket, which is what
//! makes the whole lifecycle testable against scripted responses and
//! replayable from a log.
//!
//! Latency is *returned by the probe* rather than measured by the caller
//! around the call. A caller timing the call would be reading a clock, and
//! this crate has none; a scripted probe would then report the test harness's
//! speed instead of the source's.
//!
//! Two implementations ship: [`InMemoryProbe`], which answers from a script
//! and refuses to invent anything it was not given, and [`NetworkProbe`],
//! which reports [`qip_core::Error::Unavailable`] naming exactly what
//! production has to supply. There is deliberately no third implementation
//! that "tries the network and falls back" — `qip-storage` calls that hazard
//! out and this crate honours it. A probe that quietly returned a stub would
//! let a legality assessment be made against a robots.txt nobody fetched.

use crate::endpoint::SourceEndpoint;
use crate::robots::RobotsPolicy;
use crate::schema::SourceSchema;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

/// The result of asking a host for its robots.txt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "robots", rename_all = "snake_case")]
pub enum RobotsFetch {
    /// The host served a robots.txt.
    Served { body: String, latency: Duration },
    /// The host answered and has no robots.txt.
    ///
    /// Not the same as permission. The status is kept because a 404 and a 403
    /// mean different things about a publisher's intent.
    Absent { status: u16, latency: Duration },
    /// The host could not be asked.
    Unreachable { reason: String },
}

impl RobotsFetch {
    pub fn policy(&self) -> Option<RobotsPolicy> {
        match self {
            Self::Served { body, .. } => Some(RobotsPolicy::parse(body)),
            Self::Absent { .. } | Self::Unreachable { .. } => None,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Served { body, .. } => format!("robots.txt served, {} bytes", body.len()),
            Self::Absent { status, .. } => format!("no robots.txt (HTTP {status})"),
            Self::Unreachable { reason } => format!("robots.txt could not be fetched: {reason}"),
        }
    }
}

/// The answer to a HEAD of a candidate endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadResponse {
    pub status: u16,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    /// What the source says about when its content last changed.
    pub last_modified: Option<Timestamp>,
    pub latency: Duration,
}

impl HeadResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// A body actually read from the candidate.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadSample {
    pub body: String,
    pub media_type: String,
    /// When the newest record in the body was true in the world, where the
    /// payload says. `None` means the payload carries no time of its own,
    /// which is a freshness finding rather than a parse failure.
    pub payload_at: Option<Timestamp>,
    pub latency: Duration,
}

/// The port. Four questions, no state, no clock of its own.
pub trait SourceProbe: std::fmt::Debug {
    /// Fetch `host`'s robots.txt.
    fn robots(&mut self, host: &str, at: Timestamp) -> Result<RobotsFetch>;

    /// Ask whether the endpoint is there and what it would serve.
    fn head(&mut self, endpoint: &SourceEndpoint, at: Timestamp) -> Result<HeadResponse>;

    /// Read one payload, so its shape can be fingerprinted.
    fn sample(&mut self, endpoint: &SourceEndpoint, at: Timestamp) -> Result<PayloadSample>;
}

/// Everything one probing of a candidate produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProbeEvidence {
    robots: RobotsFetch,
    robots_policy: Option<RobotsPolicy>,
    head: HeadResponse,
    sample: PayloadSample,
    schema: SourceSchema,
    observed_at: Timestamp,
}

impl ProbeEvidence {
    /// Run the whole probe against one endpoint.
    ///
    /// robots.txt is fetched before the endpoint is read, and only for
    /// mechanisms robots.txt governs. Reading the payload first and asking
    /// permission afterwards would make the check ceremonial.
    pub fn gather(
        probe: &mut dyn SourceProbe,
        endpoint: &SourceEndpoint,
        at: Timestamp,
    ) -> Result<Self> {
        let robots = if endpoint.mechanism().is_governed_by_robots() {
            probe.robots(endpoint.host(), at)?
        } else {
            RobotsFetch::Absent {
                status: 0,
                latency: Duration::ZERO,
            }
        };
        let robots_policy = robots.policy();
        let head = probe.head(endpoint, at)?;
        let sample = probe.sample(endpoint, at)?;
        let schema = SourceSchema::parse(&sample.body).unwrap_or_else(|_| {
            // A payload that is not JSON still has a shape; recording it as
            // an empty schema keeps the source assessable and makes the
            // absence visible in the fingerprint rather than aborting the
            // lifecycle over a format this phase cannot parse.
            SourceSchema::from_fields([])
        });
        Ok(Self {
            robots,
            robots_policy,
            head,
            sample,
            schema,
            observed_at: at,
        })
    }

    pub fn robots(&self) -> &RobotsFetch {
        &self.robots
    }

    pub fn robots_policy(&self) -> Option<&RobotsPolicy> {
        self.robots_policy.as_ref()
    }

    pub fn head(&self) -> &HeadResponse {
        &self.head
    }

    pub fn sample(&self) -> &PayloadSample {
        &self.sample
    }

    pub fn schema(&self) -> &SourceSchema {
        &self.schema
    }

    pub fn observed_at(&self) -> Timestamp {
        self.observed_at
    }

    /// The slower of the two measured round trips, used as the source's
    /// expected latency.
    pub fn observed_latency(&self) -> Duration {
        self.head.latency.max(self.sample.latency)
    }
}

/// A probe that answers from a script.
///
/// Successive responses for the same key are consumed in order until one
/// remains, which then repeats. That is what lets a test express "this source
/// served shape A and then shape B" without the probe having to model time.
#[derive(Debug, Default)]
pub struct InMemoryProbe {
    robots: BTreeMap<String, VecDeque<RobotsFetch>>,
    heads: BTreeMap<String, VecDeque<HeadResponse>>,
    samples: BTreeMap<String, VecDeque<PayloadSample>>,
    calls: Vec<String>,
}

impl InMemoryProbe {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_robots(mut self, host: &str, fetch: RobotsFetch) -> Self {
        self.robots
            .entry(host.to_ascii_lowercase())
            .or_default()
            .push_back(fetch);
        self
    }

    pub fn with_head(mut self, url: &str, response: HeadResponse) -> Self {
        self.heads
            .entry(url.to_string())
            .or_default()
            .push_back(response);
        self
    }

    pub fn with_sample(mut self, url: &str, sample: PayloadSample) -> Self {
        self.samples
            .entry(url.to_string())
            .or_default()
            .push_back(sample);
        self
    }

    /// Every call made, in order. A test asserting that a denylisted host was
    /// never contacted reads this.
    pub fn calls(&self) -> &[String] {
        &self.calls
    }

    fn take<T: Clone>(queue: &mut VecDeque<T>) -> Option<T> {
        if queue.len() > 1 {
            queue.pop_front()
        } else {
            queue.front().cloned()
        }
    }
}

impl SourceProbe for InMemoryProbe {
    fn robots(&mut self, host: &str, _at: Timestamp) -> Result<RobotsFetch> {
        let host = host.to_ascii_lowercase();
        self.calls.push(format!("robots {host}"));
        let Some(queue) = self.robots.get_mut(&host) else {
            return Err(Error::not_found(format!(
                "the scripted probe has no robots.txt for `{host}`; it will not invent one, \
                 because a legality verdict against an invented robots.txt is worse than no \
                 verdict"
            )));
        };
        Self::take(queue).ok_or_else(|| {
            Error::not_found(format!("the scripted robots.txt for `{host}` is exhausted"))
        })
    }

    fn head(&mut self, endpoint: &SourceEndpoint, _at: Timestamp) -> Result<HeadResponse> {
        let url = endpoint.url();
        self.calls.push(format!("head {url}"));
        let Some(queue) = self.heads.get_mut(&url) else {
            return Err(Error::not_found(format!(
                "the scripted probe has no HEAD response for `{url}`"
            )));
        };
        Self::take(queue).ok_or_else(|| {
            Error::not_found(format!("the scripted HEAD responses for `{url}` are exhausted"))
        })
    }

    fn sample(&mut self, endpoint: &SourceEndpoint, _at: Timestamp) -> Result<PayloadSample> {
        let url = endpoint.url();
        self.calls.push(format!("sample {url}"));
        let Some(queue) = self.samples.get_mut(&url) else {
            return Err(Error::not_found(format!(
                "the scripted probe has no payload sample for `{url}`"
            )));
        };
        Self::take(queue).ok_or_else(|| {
            Error::not_found(format!("the scripted samples for `{url}` are exhausted"))
        })
    }
}

/// The probe a deployment would use, and cannot yet.
///
/// This build links no HTTP transport and holds no credentials, so every
/// method reports [`Error::Unavailable`] naming what is missing. It exists as
/// a type rather than as a gap in the documentation so that wiring a
/// deployment to the network fails at the first probe, in the process that
/// was misconfigured, rather than producing a plausible empty result.
#[derive(Debug, Default)]
pub struct NetworkProbe {
    user_agent: Option<String>,
    egress_policy: Option<String>,
    tls_trust_roots: Option<String>,
    credentials: BTreeMap<String, String>,
}

impl NetworkProbe {
    /// The transport requirement, which no amount of configuration satisfies
    /// in this phase.
    pub const TRANSPORT_REQUIREMENT: &'static str =
        "an HTTP/1.1 client with TLS 1.2+ and certificate verification (no transport is linked \
         into this build; see docs/adr/0009-tiered-dependency-policy.md)";

    pub fn unconfigured() -> Self {
        Self::default()
    }

    /// Name the crawler. A publisher's only means of asking us to stop is to
    /// block a user agent, so an anonymous crawler is one that cannot be told
    /// no.
    pub fn identified_as(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }

    /// Declare which egress the probe may leave through.
    pub fn through_egress(mut self, policy: impl Into<String>) -> Self {
        self.egress_policy = Some(policy.into());
        self
    }

    pub fn trusting(mut self, trust_roots: impl Into<String>) -> Self {
        self.tls_trust_roots = Some(trust_roots.into());
        self
    }

    pub fn with_credential(mut self, host: impl Into<String>, reference: impl Into<String>) -> Self {
        self.credentials.insert(host.into(), reference.into());
        self
    }

    /// Everything production must supply before this probe can work.
    ///
    /// The transport requirement is always present: it is a build-time fact,
    /// not a configuration value, and a list that could empty itself would
    /// suggest this probe becomes usable once the environment is right.
    pub fn missing_configuration(&self) -> Vec<String> {
        let mut missing = vec![Self::TRANSPORT_REQUIREMENT.to_string()];
        if self.egress_policy.is_none() {
            missing.push(
                "an outbound egress policy naming which hosts and ports the crawler may reach"
                    .to_string(),
            );
        }
        if self.user_agent.is_none() {
            missing.push(
                "a user-agent identity, so a publisher can identify and block this crawler"
                    .to_string(),
            );
        }
        if self.tls_trust_roots.is_none() {
            missing.push("a TLS trust root bundle".to_string());
        }
        if self.credentials.is_empty() {
            missing.push(
                "per-host credentials for the sources that require authentication".to_string(),
            );
        }
        missing
    }

    fn unavailable(&self, attempted: &str) -> Error {
        Error::unavailable(format!(
            "the network probe cannot {attempted}. It requires: {}",
            self.missing_configuration().join("; ")
        ))
    }
}

impl SourceProbe for NetworkProbe {
    fn robots(&mut self, host: &str, _at: Timestamp) -> Result<RobotsFetch> {
        Err(self.unavailable(&format!("fetch https://{host}/robots.txt")))
    }

    fn head(&mut self, endpoint: &SourceEndpoint, _at: Timestamp) -> Result<HeadResponse> {
        Err(self.unavailable(&format!("HEAD {}", endpoint.url())))
    }

    fn sample(&mut self, endpoint: &SourceEndpoint, _at: Timestamp) -> Result<PayloadSample> {
        Err(self.unavailable(&format!("sample {}", endpoint.url())))
    }
}
