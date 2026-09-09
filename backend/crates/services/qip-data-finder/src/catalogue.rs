//! The committed list of candidate sources, and the routes they are reached
//! through (ADR 0054).
//!
//! # Why a file and not a crawl
//!
//! The egress proxy is a reverse proxy: a process picks a destination by
//! picking a loopback port, and cannot name a host, because the request carries
//! no field in which a host could be named. So the set of sources this platform
//! can reach is exactly the set somebody wrote an Envoy cluster for — and this
//! file is where that set is stated in the platform's own vocabulary, beside
//! the route each entry travels.
//!
//! # The refusal that matters
//!
//! **An entry whose route is missing or malformed is refused at load, by name.**
//! ADR 0054 names the alternative as one of the two things that would void the
//! decision: a catalogue that accepted such an entry would produce a run in
//! which some candidates were assessed and others were "unreachable" for a
//! reason that reads as the publisher's fault and is ours. A refusal here is
//! loud, names the source, and happens before any socket is opened.

use crate::source::SourceCandidate;
use qip_core::error::{Error, Result};
use qip_core::time::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// One candidate, and the reviewed egress route it is probed through.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateEntry {
    /// The candidate in the platform's own vocabulary.
    pub candidate: SourceCandidate,
    /// The loopback base URL of the reviewed egress route for this source's
    /// host — an `http://` address, because the client has no TLS stack and the
    /// proxy behind the route originates TLS upstream.
    ///
    /// Named per entry rather than derived from the endpoint, because the two
    /// are different facts: the endpoint says where the data lives, and this
    /// says which reviewed door the platform is allowed to reach it through.
    /// Deriving one from the other would be inventing a route.
    pub egress_route: String,
}

/// A catalogue as loaded, with the digest of the bytes it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedCandidates {
    pub entries: Vec<CandidateEntry>,
    /// SHA-256 of the file, so a run can say which catalogue it assessed
    /// against — the same discipline the instrument catalogue follows.
    pub digest: String,
}

impl LoadedCandidates {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Parse and validate a committed candidate catalogue.
///
/// `now` is the instant the load happens at, and a candidate claiming to have
/// been discovered *after* it is refused: a discovery instant in the future is
/// either a clock the platform cannot trust or an entry backdated the wrong
/// way, and both make the bitemporal record wrong in the direction that hides
/// a leak.
pub fn load(text: &str, now: Timestamp) -> Result<LoadedCandidates> {
    let entries: Vec<CandidateEntry> = serde_json::from_str(text).map_err(|error| {
        Error::invalid(format!("the candidate catalogue is not valid: {error}"))
    })?;
    if entries.is_empty() {
        return Err(Error::invalid(
            "the candidate catalogue is empty. An empty catalogue and a catalogue nobody \
             mounted produce the same silent run, so this is refused rather than treated as \
             `no sources to assess`",
        ));
    }

    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for entry in &entries {
        let id = entry.candidate.id();
        // Serde wrote the candidate's private fields directly, so the
        // constructor's guards never ran. This is where they run.
        entry.candidate.validate()?;
        if !seen.insert(id) {
            return Err(Error::invalid(format!(
                "the candidate catalogue names `{id}` twice; two entries under one id is a \
                 record that cannot say which route a decision was reached through"
            )));
        }
        validate_route(id, &entry.egress_route)?;
        if entry.candidate.discovered_at() > now {
            return Err(Error::invalid(format!(
                "candidate `{id}` records a discovery instant after the load instant. A source \
                 discovered in the future is a clock nobody can trust, and it is refused rather \
                 than clamped"
            )));
        }
    }

    Ok(LoadedCandidates {
        digest: qip_core::sha256_hex(text.as_bytes()),
        entries,
    })
}

/// A route must be an `http://` loopback address of a reviewed egress listener.
///
/// `https` is refused by name rather than downgraded, for the reason
/// [`crate::probe::NetworkProbe::through`] gives: downgrading would send a
/// plaintext request to port 443. A route naming no scheme is refused rather
/// than having one prepended, because a prepended scheme is a guess about which
/// door was meant.
fn validate_route(id: &str, route: &str) -> Result<()> {
    let route = route.trim();
    if route.is_empty() {
        return Err(Error::invalid(format!(
            "candidate `{id}` names no egress route. Under ADR 0054 a source is probed only \
             where a reviewed route exists, and an entry without one would be attempted and \
             fail at the socket for a reason that reads as the publisher's fault"
        )));
    }
    if route.starts_with("https://") {
        return Err(Error::invalid(format!(
            "candidate `{id}` names the https route `{route}`. A route is the loopback address \
             of an egress listener, and the proxy behind it originates TLS upstream; this \
             client has no TLS stack and would send plaintext to port 443"
        )));
    }
    if !route.starts_with("http://") {
        return Err(Error::invalid(format!(
            "candidate `{id}` names `{route}`, which is not the `http://` address of an egress \
             route"
        )));
    }
    Ok(())
}
