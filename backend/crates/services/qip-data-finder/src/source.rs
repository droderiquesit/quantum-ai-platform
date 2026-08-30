//! A candidate before it was probed, and a source after it was.
//!
//! These are two types rather than one type with an `Option<Evidence>`, and
//! that is the point. A [`SourceCandidate`] holds claims — what a directory
//! said, what a vendor's marketing page promised. A [`Source`] holds those
//! claims *plus* what a probe observed, and there is no constructor for one
//! that does not take evidence. A function that must not run on hearsay takes
//! a `&Source` and the compiler enforces the rest.
//!
//! The failure this prevents is specific: a `probed: bool` field that is
//! false, a check that forgets to read it, and a registration decision made
//! against a schema nobody fetched.

use crate::coverage::{SourceCoverage, SourceRegion};
use crate::endpoint::SourceEndpoint;
use crate::legal::LicensingPosture;
use crate::probe::ProbeEvidence;
use crate::quality::SourceCost;
use crate::schema::SourceSchema;
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_events::Topic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Who a source is.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceIdentity {
    id: String,
    name: String,
    publisher: String,
}

impl SourceIdentity {
    /// Identify a source.
    ///
    /// The publisher is required. A source with no named publisher is one
    /// nobody can be asked for terms, served a takedown by, or invoiced —
    /// which makes every later legal question unanswerable by construction.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        publisher: impl Into<String>,
    ) -> Result<Self> {
        let id = id.into();
        let name = name.into();
        let publisher = publisher.into();
        if id.trim().is_empty() {
            return Err(Error::invalid("a source must have a stable identifier"));
        }
        if name.trim().is_empty() {
            return Err(Error::invalid(format!("source `{id}` must have a name")));
        }
        if publisher.trim().is_empty() {
            return Err(Error::invalid(format!(
                "source `{id}` must name its publisher; an unattributed source cannot be asked \
                 for terms and cannot be held to any"
            )));
        }
        Ok(Self {
            id,
            name,
            publisher,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn publisher(&self) -> &str {
        &self.publisher
    }
}

/// A source that has been found and not yet checked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceCandidate {
    identity: SourceIdentity,
    endpoint: SourceEndpoint,
    /// What the source *claims* to cover. Unverified by construction.
    declared_coverage: SourceCoverage,
    /// What is known about its licensing before anything is fetched.
    declared_licensing: LicensingPosture,
    cost: SourceCost,
    region: SourceRegion,
    /// The record types an adapter over this source would publish.
    produces: BTreeSet<Topic>,
    /// Where the candidate came from — a directory, a sitemap, an operator.
    discovered_from: String,
    discovered_at: Timestamp,
}

impl SourceCandidate {
    pub fn new(
        identity: SourceIdentity,
        endpoint: SourceEndpoint,
        declared_coverage: SourceCoverage,
        declared_licensing: LicensingPosture,
        cost: SourceCost,
        region: SourceRegion,
        produces: impl IntoIterator<Item = Topic>,
        discovered_from: impl Into<String>,
        discovered_at: Timestamp,
    ) -> Result<Self> {
        let discovered_from = discovered_from.into();
        if discovered_from.trim().is_empty() {
            return Err(Error::invalid(format!(
                "candidate `{}` must record where it was discovered; a source that appeared \
                 from nowhere cannot be re-derived when its decision is questioned",
                identity.id()
            )));
        }
        let produces: BTreeSet<Topic> = produces.into_iter().collect();
        if produces.is_empty() {
            return Err(Error::invalid(format!(
                "candidate `{}` must declare what it produces, or no adapter can publish it",
                identity.id()
            )));
        }
        Ok(Self {
            identity,
            endpoint,
            declared_coverage,
            declared_licensing,
            cost,
            region,
            produces,
            discovered_from,
            discovered_at,
        })
    }

    pub fn identity(&self) -> &SourceIdentity {
        &self.identity
    }

    pub fn id(&self) -> &str {
        self.identity.id()
    }

    pub fn endpoint(&self) -> &SourceEndpoint {
        &self.endpoint
    }

    /// Coverage as claimed, not as observed.
    pub fn declared_coverage(&self) -> &SourceCoverage {
        &self.declared_coverage
    }

    pub fn declared_licensing(&self) -> &LicensingPosture {
        &self.declared_licensing
    }

    pub fn cost(&self) -> &SourceCost {
        &self.cost
    }

    pub fn region(&self) -> SourceRegion {
        self.region
    }

    pub fn produces(&self) -> &BTreeSet<Topic> {
        &self.produces
    }

    pub fn discovered_from(&self) -> &str {
        &self.discovered_from
    }

    pub fn discovered_at(&self) -> Timestamp {
        self.discovered_at
    }
}

/// A candidate plus what a probe actually saw.
///
/// Constructed only by [`Source::from_evidence`], which needs a
/// [`ProbeEvidence`], which can only come from a [`crate::probe::SourceProbe`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    candidate: SourceCandidate,
    evidence: ProbeEvidence,
}

impl Source {
    /// Bind a candidate to the evidence gathered about it.
    pub fn from_evidence(candidate: SourceCandidate, evidence: ProbeEvidence) -> Self {
        Self {
            candidate,
            evidence,
        }
    }

    pub fn candidate(&self) -> &SourceCandidate {
        &self.candidate
    }

    pub fn id(&self) -> &str {
        self.candidate.id()
    }

    pub fn identity(&self) -> &SourceIdentity {
        self.candidate.identity()
    }

    pub fn endpoint(&self) -> &SourceEndpoint {
        self.candidate.endpoint()
    }

    pub fn coverage(&self) -> &SourceCoverage {
        self.candidate.declared_coverage()
    }

    pub fn licensing(&self) -> &LicensingPosture {
        self.candidate.declared_licensing()
    }

    pub fn cost(&self) -> &SourceCost {
        self.candidate.cost()
    }

    pub fn region(&self) -> SourceRegion {
        self.candidate.region()
    }

    pub fn evidence(&self) -> &ProbeEvidence {
        &self.evidence
    }

    /// The shape the source was actually serving when probed.
    pub fn schema(&self) -> &SourceSchema {
        self.evidence.schema()
    }

    pub fn probed_at(&self) -> Timestamp {
        self.evidence.observed_at()
    }
}

/// Where a source came from and what happened to it.
///
/// Registered into the mesh catalogue's lineage rather than kept here as a
/// second copy: this records the finder's own steps, which the catalogue has
/// no concept of, and hands the catalogue the parent link it does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLineage {
    source_id: String,
    discovered_from: String,
    discovered_at: Timestamp,
    probed_at: Timestamp,
    decided_at: Timestamp,
    /// The source this one was found to replace, if it was found that way.
    replaces: Option<String>,
}

impl SourceLineage {
    pub fn new(source: &Source, decided_at: Timestamp) -> Self {
        Self {
            source_id: source.id().to_string(),
            discovered_from: source.candidate().discovered_from().to_string(),
            discovered_at: source.candidate().discovered_at(),
            probed_at: source.probed_at(),
            decided_at,
            replaces: None,
        }
    }

    pub fn replacing(mut self, replaced: impl Into<String>) -> Self {
        self.replaces = Some(replaced.into());
        self
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn discovered_from(&self) -> &str {
        &self.discovered_from
    }

    pub fn discovered_at(&self) -> Timestamp {
        self.discovered_at
    }

    pub fn probed_at(&self) -> Timestamp {
        self.probed_at
    }

    pub fn decided_at(&self) -> Timestamp {
        self.decided_at
    }

    pub fn replaces(&self) -> Option<&str> {
        self.replaces.as_deref()
    }

    /// The parent datasets, in the mesh catalogue's vocabulary.
    pub fn produced_from(&self) -> Vec<String> {
        match &self.replaces {
            Some(replaced) => vec![format!("source.{replaced}")],
            None => Vec::new(),
        }
    }
}
