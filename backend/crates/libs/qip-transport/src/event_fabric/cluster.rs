//! The fabric client's view of the cluster: bootstrap from SRV, fetch the live
//! partition map, follow a leader change (ADR 0135 decisions 4 and 5,
//! R12-FABRIC-8098).
//!
//! Until this module every client held one endpoint by configuration, so a
//! broker replaced in a region was found by redeploying every client, and a
//! producer whose partition moved kept dialling the old leader until a person
//! noticed. Here a client:
//!
//! 1. resolves the broker authorities through [`Bootstrap`] — the SRV answer,
//!    the last good one while it is fresh, then the configured addresses;
//! 2. asks one of them for the [`ClusterMap`] **before its first request**,
//!    so it never produces on a guess; and
//! 3. on a transport failure re-resolves, re-fetches the map and retries
//!    once at the leader the new map names — refreshed on a failure, not on
//!    a timer.
//!
//! # Discovery is never trust
//!
//! The map names an address; the connection to it is still authenticated by
//! whatever the `connect` seam builds (mTLS in every non-local deployment,
//! ADR 0112). A wrong map can send a client somewhere that refuses it. It
//! cannot make the client talk to an unauthenticated broker.
//!
//! # What a retry does not do
//!
//! One retry, at a different authority only. A refresh that names the same
//! leader that just failed returns the failure: retrying the same address
//! would only repeat it, and the caller's own breaker and backoff are what
//! pace that. A leader change also gives the broker a new producer epoch, so
//! the caller's [`super::producer::Producer`] must `init` again, which its
//! fenced latch already demands.

use qip_core::error::{Error, Result};

use crate::dns::{AddressSource, Bootstrap, SrvAnswer};

use super::protocol::{ClusterMap, ClusterMapRequest, Request, Response, Route};
use super::transport::{FabricTransport, Timeouts};

/// Builds a transport to one broker's `host:port`. The composition root
/// supplies it so the link security (mTLS, or plaintext in `local`) is its
/// choice and never this module's.
pub type Connect = Box<dyn Fn(&str) -> Result<Box<dyn FabricTransport + Send>> + Send>;

/// Runs the SRV lookup. A closure rather than a [`crate::dns::SrvResolver`]
/// so a test can answer from a script, and `local` (where SRV is not used)
/// can answer with a refusal and so stay on the configured addresses.
pub type Lookup = Box<dyn FnMut() -> Result<SrvAnswer> + Send>;

impl std::fmt::Debug for ClusterClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClusterClient")
            .field("answered_by", &self.answered_by)
            .field("source", &self.source)
            .field("map", &self.map)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for LeaderFollowingTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeaderFollowingTransport")
            .field("stream", &self.stream)
            .field("partition", &self.partition)
            .field("client", &self.client)
            .finish()
    }
}

/// One client's view of the cluster. See the module documentation.
pub struct ClusterClient {
    bootstrap: Bootstrap,
    lookup: Lookup,
    connect: Connect,
    timeouts: Timeouts,
    map: Option<ClusterMap>,
    /// The authority that answered the last map fetch: where a partition the
    /// map names no address for lives.
    answered_by: Option<String>,
    source: Option<AddressSource>,
}

impl ClusterClient {
    pub fn new(bootstrap: Bootstrap, lookup: Lookup, connect: Connect, timeouts: Timeouts) -> Self {
        Self {
            bootstrap,
            lookup,
            connect,
            timeouts,
            map: None,
            answered_by: None,
            source: None,
        }
    }

    /// Which rung the last refresh's addresses came from, for a metric label.
    pub fn source(&self) -> Option<AddressSource> {
        self.source
    }

    /// The map as last fetched, if any.
    pub fn map(&self) -> Option<&ClusterMap> {
        self.map.as_ref()
    }

    /// Resolve the bootstrap addresses and fetch the map from the first that
    /// answers. A fetch that fails everywhere leaves the previous map in
    /// place and returns the last failure: a stale map is not better than
    /// none to a caller that has to decide, so it is the caller's to see.
    pub fn refresh(&mut self) -> Result<AddressSource> {
        let resolved = self.bootstrap.resolve(&mut self.lookup);
        let mut last_failure = None;
        for authority in &resolved.authorities {
            match self.fetch_from(authority) {
                Ok(map) => {
                    self.map = Some(map);
                    self.answered_by = Some(authority.clone());
                    self.source = Some(resolved.source);
                    return Ok(resolved.source);
                }
                Err(error) => {
                    last_failure = Some(format!("{authority}: {error}"));
                }
            }
        }
        Err(Error::unavailable(format!(
            "no bootstrap address ({}) answered a cluster-map request; last failure: {}",
            resolved.source.as_label(),
            last_failure.unwrap_or_else(|| "none was tried".to_string())
        )))
    }

    fn fetch_from(&self, authority: &str) -> Result<ClusterMap> {
        let mut transport = (self.connect)(authority)?;
        match transport.call(Request::ClusterMap(ClusterMapRequest {}), self.timeouts)? {
            Response::ClusterMap(map) => Ok(map),
            Response::Refused(refusal) => Err(Error::denied(format!(
                "{authority} refused the cluster-map request: {refusal:?}"
            ))),
            other => Err(Error::invalid(format!(
                "{authority} answered a cluster-map request with {:?}",
                other.route()
            ))),
        }
    }

    /// The authority to send `(stream, partition)` to, from the map. Refuses
    /// when no map has been fetched or the map does not hold the partition:
    /// a client that guessed would produce to a broker it was never told
    /// leads it.
    pub fn leader_authority(&self, stream: &str, partition: u32) -> Result<String> {
        let map = self.map.as_ref().ok_or_else(|| {
            Error::invalid("no cluster map has been fetched; refresh() before routing a request")
        })?;
        let entry = map.leader_of(stream, partition).ok_or_else(|| {
            Error::denied(format!(
                "the cluster map holds no leader for {stream}:{partition}; the caller's grants \
                 may not reach it, or the stream is not declared"
            ))
        })?;
        match (&entry.leader, &self.answered_by) {
            (Some(address), _) => Ok(address.clone()),
            (None, Some(answerer)) => Ok(answerer.clone()),
            (None, None) => Err(Error::invalid("a cluster map with no answerer")),
        }
    }

    /// Send `request` for `(stream, partition)` to its leader, fetching the
    /// map first when there is none and following one leader change on a
    /// transport failure.
    pub fn call(&mut self, stream: &str, partition: u32, request: Request) -> Result<Response> {
        if self.map.is_none() {
            self.refresh()?;
        }
        let first = self.leader_authority(stream, partition)?;
        let failure = match self.send(&first, request.clone()) {
            Ok(response) => return Ok(response),
            Err(error) => error,
        };
        // The leader may have moved. Re-resolve and re-read the map, and try
        // again only if it names somewhere else.
        if self.refresh().is_err() {
            return Err(failure);
        }
        let second = self.leader_authority(stream, partition)?;
        if second == first {
            return Err(failure);
        }
        self.send(&second, request)
    }

    fn send(&self, authority: &str, request: Request) -> Result<Response> {
        let mut transport = (self.connect)(authority)?;
        transport.call(request, self.timeouts)
    }
}

/// A [`FabricTransport`] for one `(stream, partition)` that routes every call
/// through a [`ClusterClient`]. It is what a producer or consumer is handed in
/// place of a single-endpoint transport, so the map is fetched before its
/// first call and a leader change is followed.
pub struct LeaderFollowingTransport {
    client: ClusterClient,
    stream: String,
    partition: u32,
}

impl LeaderFollowingTransport {
    pub fn new(client: ClusterClient, stream: impl Into<String>, partition: u32) -> Self {
        Self {
            client,
            stream: stream.into(),
            partition,
        }
    }

    /// The client underneath, for a caller that reads the map or its source.
    pub fn client(&self) -> &ClusterClient {
        &self.client
    }
}

impl FabricTransport for LeaderFollowingTransport {
    fn call(&mut self, request: Request, _timeouts: Timeouts) -> Result<Response> {
        // The cluster-map request itself has no partition and is the
        // client's own business; one arriving here would loop through the
        // routing it exists to build.
        if request.route() == Route::ClusterMap {
            return Err(Error::invalid(
                "a cluster-map request was sent through a leader-following transport; call \
                 ClusterClient::refresh instead",
            ));
        }
        self.client.call(&self.stream, self.partition, request)
    }
}
