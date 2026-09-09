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
use qip_transport::http::{ClientLimits, HttpClient, HttpRequest, HttpResponse, Method};
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
            Error::not_found(format!(
                "the scripted HEAD responses for `{url}` are exhausted"
            ))
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

/// The probe a deployment uses, reaching one source through one egress route.
///
/// # How it reaches anything (ADR 0054)
///
/// **Not by naming a host.** [`qip_transport::http`] emits an origin-form
/// request line and a `host:` header carrying whatever authority its base URL
/// had; it never emits `CONNECT` and never an absolute-form URI. The egress
/// proxy in front of it is a *reverse* proxy whose listeners each bind a
/// loopback port meaning exactly one upstream, so the destination is a property
/// of the socket rather than a value this process supplies. A probe cannot
/// reach a host by asking for it, because the request has no field in which a
/// host can be asked for.
///
/// So this type is constructed with the **base URL of the egress route** for
/// one source, exactly as `qip_storage::gcp` and the Frankfurter connector are.
/// It is not a crawler and cannot become one: pointing it at a host with no
/// reviewed route produces a connection refused on loopback, not a fetch.
///
/// # What it still refuses
///
/// Every method needs a user agent, because a publisher's only way to say no is
/// to block one, and an anonymous crawler is one that cannot be told no. That
/// refusal is a construction-time check rather than a per-call one, so a
/// misconfigured deployment fails at assembly rather than at the first source.
///
/// It carries no credential. The `Registered` and `Licensed` access modes
/// `tier.rs` models therefore stay unreachable, and that is deliberate: a
/// credential in a discovery path is a credential in a process whose whole job
/// is to touch things nobody has vetted.
#[derive(Debug)]
pub struct NetworkProbe {
    /// The loopback base URL of this source's reviewed egress route, without a
    /// trailing slash.
    base_url: String,
    user_agent: String,
    client: HttpClient,
}

impl NetworkProbe {
    /// Build a probe against one reviewed egress route.
    ///
    /// Refuses an `https` base URL by name rather than downgrading it. The
    /// client has no TLS stack; the proxy originates TLS upstream. A caller
    /// handing an `https` URL has confused the route with the destination, and
    /// the two are the whole distinction this type rests on.
    pub fn through(base_url: impl Into<String>, user_agent: impl Into<String>) -> Result<Self> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let user_agent = user_agent.into();
        if user_agent.trim().is_empty() {
            return Err(Error::invalid(
                "a source probe needs a user agent: a publisher's only means of asking this \
                 platform to stop is to block one, so an anonymous probe is one that cannot be \
                 told no",
            ));
        }
        if base_url.starts_with("https://") {
            return Err(Error::invalid(format!(
                "the probe base URL `{base_url}` is https, and this client has no TLS stack. It \
                 addresses the loopback egress route for one source; the proxy behind that route \
                 originates TLS upstream. See ADR 0054"
            )));
        }
        if !base_url.starts_with("http://") {
            return Err(Error::invalid(format!(
                "the probe base URL `{base_url}` names no scheme this client speaks; it must be \
                 the `http://` address of a reviewed egress route"
            )));
        }
        Ok(Self {
            base_url,
            user_agent,
            client: HttpClient::new(ClientLimits::default()),
        })
    }

    /// The route this probe is bound to.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Send one request through the route, carrying the user agent.
    fn fetch(&self, method: Method, path: &str) -> std::result::Result<HttpResponse, String> {
        let url = format!("{}{}", self.base_url, path);
        let request = HttpRequest::new(method, &url)
            .map_err(|error| error.to_string())?
            .with_header("user-agent", &self.user_agent)
            // Named so a publisher reading its own logs can tell a discovery
            // probe from an ingestion poll without correlating timestamps.
            .with_header("accept", "*/*");
        self.client
            .send(&request)
            .map_err(|error| error.to_string())
    }
}

impl SourceProbe for NetworkProbe {
    /// Ask the route's host for its robots.txt.
    ///
    /// `host` is not used to choose a destination — it cannot be; see the type
    /// documentation — and a mismatch between it and the route is a
    /// configuration error the catalogue is responsible for refusing. It is
    /// carried into the refusal text so that error names the source rather than
    /// a port number.
    fn robots(&mut self, host: &str, _at: Timestamp) -> Result<RobotsFetch> {
        let started = std::time::Instant::now();
        match self.fetch(Method::Get, "/robots.txt") {
            Ok(response) if response.is_success() => match response.body_as_str() {
                Ok(body) => Ok(RobotsFetch::Served {
                    body: body.to_string(),
                    latency: elapsed(started),
                }),
                // A robots.txt that is not UTF-8 is not a robots.txt. Treated
                // as unreachable rather than as absent: absent means the host
                // answered and has no policy, and this host answered with
                // something nobody can read, which is a different fact.
                Err(error) => Ok(RobotsFetch::Unreachable {
                    reason: format!("{host} served an undecodable robots.txt: {error}"),
                }),
            },
            Ok(response) => Ok(RobotsFetch::Absent {
                status: response.status,
                latency: elapsed(started),
            }),
            Err(reason) => Ok(RobotsFetch::Unreachable {
                reason: format!("{host} via {}: {reason}", self.base_url),
            }),
        }
    }

    fn head(&mut self, endpoint: &SourceEndpoint, _at: Timestamp) -> Result<HeadResponse> {
        let started = std::time::Instant::now();
        let response = self
            .fetch(Method::Head, endpoint.path())
            .map_err(|reason| {
                Error::unavailable(format!(
                    "HEAD {} through {} failed: {reason}",
                    endpoint.url(),
                    self.base_url
                ))
            })?;
        Ok(HeadResponse {
            status: response.status,
            content_type: response.header("content-type").map(str::to_string),
            content_length: response
                .header("content-length")
                .and_then(|value| value.parse().ok()),
            // Deliberately not parsed. `Last-Modified` is an RFC 7231 date and
            // this crate has no date parser; inventing one to fill a field
            // whose `None` already means "the source said nothing about when
            // its content changed" would trade a known gap for a guess. The
            // freshness finding comes from the payload's own timestamp.
            last_modified: None,
            latency: elapsed(started),
        })
    }

    fn sample(&mut self, endpoint: &SourceEndpoint, _at: Timestamp) -> Result<PayloadSample> {
        let started = std::time::Instant::now();
        let response = self.fetch(Method::Get, endpoint.path()).map_err(|reason| {
            Error::unavailable(format!(
                "sampling {} through {} failed: {reason}",
                endpoint.url(),
                self.base_url
            ))
        })?;
        if !response.is_success() {
            return Err(Error::unavailable(format!(
                "sampling {} through {} answered HTTP {}: a non-2xx body is not a payload, and \
                 fingerprinting an error page as a schema is how a source is admitted on the \
                 shape of its own 404",
                endpoint.url(),
                self.base_url,
                response.status
            )));
        }
        let body = response
            .body_as_str()
            .map_err(|error| {
                Error::invalid(format!(
                    "the payload from {} is not valid UTF-8: {error}",
                    endpoint.url()
                ))
            })?
            .to_string();
        Ok(PayloadSample {
            media_type: response
                .header("content-type")
                // The bare type, without the charset parameter: the schema
                // fingerprint keys on `application/json`, and
                // `application/json; charset=utf-8` is the same media type
                // wearing a parameter.
                .and_then(|value| value.split(';').next())
                .unwrap_or("application/octet-stream")
                .trim()
                .to_string(),
            body,
            // As with `last_modified`: the payload's own instant needs a parser
            // that knows this source's shape, which is the adapter's job and
            // not the probe's. `None` is a freshness finding, and `assess`
            // treats it as one.
            payload_at: None,
            latency: elapsed(started),
        })
    }
}

/// Wall-clock elapsed since `started`, as the platform's own `Duration`.
///
/// A measurement rather than a clock reading: the probe is handed the
/// simulation instant for everything it *records*, and latency is the one thing
/// only the wall clock can answer.
fn elapsed(started: std::time::Instant) -> Duration {
    Duration::from_nanos(started.elapsed().as_nanos().min(i64::MAX as u128) as i64)
}
