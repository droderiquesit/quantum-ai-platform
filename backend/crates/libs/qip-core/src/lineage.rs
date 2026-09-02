//! Decision lineage.
//!
//! Section 24 of the platform charter requires that every proposed investment
//! decision be reconstructable from observation through to learning. That is
//! implemented by threading a [`CorrelationId`] from the originating market
//! observation into every downstream event, and a [`CausationId`] naming the
//! immediate parent. Walking the event log by correlation id yields the full
//! chain; walking by causation id yields the exact tree.

use crate::ids::{EventId, Id};
use serde::{Deserialize, Serialize};
use std::fmt;

crate::id_kind! {
    CorrelationKind => "cor",
}

/// Groups every event produced while working one originating observation.
pub type CorrelationId = Id<CorrelationKind>;

/// The event that directly produced this one.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CausationId(pub String);

impl CausationId {
    pub fn from_event(id: &EventId) -> Self {
        Self(id.as_str().to_string())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CausationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "caused_by:{}", self.0)
    }
}

/// Distributed-trace identity, exported to OpenTelemetry.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TraceId(pub String);

impl TraceId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "trace:{}", self.0)
    }
}

/// Provenance attached to every event and every derived record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lineage {
    /// Shared by every event tracing back to the same originating observation.
    pub correlation_id: CorrelationId,
    /// The immediate parent event, absent for a root observation.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub causation_id: Option<CausationId>,
    /// Distributed trace, when the work happened inside a traced request.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub trace_id: Option<TraceId>,
    /// Component that produced the record, e.g. `opportunity-engine`.
    pub producer: String,
}

impl Lineage {
    /// Start a new chain from an originating observation.
    pub fn root(correlation_id: CorrelationId, producer: impl Into<String>) -> Self {
        Self {
            correlation_id,
            causation_id: None,
            trace_id: None,
            producer: producer.into(),
        }
    }

    /// Continue an existing chain, recording `parent` as the direct cause.
    pub fn caused_by(&self, parent: &EventId, producer: impl Into<String>) -> Self {
        Self {
            correlation_id: self.correlation_id.clone(),
            causation_id: Some(CausationId::from_event(parent)),
            trace_id: self.trace_id.clone(),
            producer: producer.into(),
        }
    }

    /// Continue the chain from the same parent but a different producer.
    pub fn relabel(&self, producer: impl Into<String>) -> Self {
        Self {
            producer: producer.into(),
            ..self.clone()
        }
    }
}
