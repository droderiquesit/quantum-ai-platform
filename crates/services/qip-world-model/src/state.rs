//! World state and what changed.
//!
//! The charter asks the platform to answer two questions: what does it believe
//! the state of the world to be, and what changed between two instants. The
//! second is the interesting one — it is what the discovery stage watches, and
//! it is only well defined if the state can be reconstructed at an arbitrary
//! past moment, which bitemporality provides.

use qip_core::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// What kind of change occurred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    EntityAdded,
    EntityUpdated,
    RelationshipAdded,
    RelationshipRetracted,
    FeatureMoved,
    CausalClaimAdded,
    /// A previously believed fact turned out to be wrong.
    BeliefRevised,
}

/// One difference between two world states.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub kind: ChangeKind,
    /// Node or relationship the change concerns.
    pub subject: String,
    pub description: String,
    /// How significant the change is, in `[0, 1]`. Drives what the discovery
    /// stage bothers to look at.
    pub materiality: f64,
    pub at: Timestamp,
}

impl Change {
    pub fn new(
        kind: ChangeKind,
        subject: impl Into<String>,
        description: impl Into<String>,
        materiality: f64,
        at: Timestamp,
    ) -> Self {
        Self {
            kind,
            subject: subject.into(),
            description: description.into(),
            materiality: materiality.clamp(0.0, 1.0),
            at,
        }
    }
}

/// A snapshot of what the platform believed at one instant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldState {
    /// The instant the state describes.
    pub valid_at: Timestamp,
    /// The instant the beliefs were held.
    pub known_at: Timestamp,
    pub entity_count: usize,
    pub object_count: usize,
    pub relationship_count: usize,
    pub causal_claim_count: usize,
    /// Feature values keyed by `feature/subject`.
    pub features: BTreeMap<String, f64>,
    /// The most connected nodes, with their degrees.
    pub hubs: Vec<(String, usize)>,
}

impl WorldState {
    /// A one-line summary, for logs and the operator surface.
    pub fn summary(&self) -> String {
        format!(
            "as of {} (known at {}): {} entities, {} objects, {} relationships, {} causal claims, \
             {} feature values",
            self.valid_at.to_rfc3339(),
            self.known_at.to_rfc3339(),
            self.entity_count,
            self.object_count,
            self.relationship_count,
            self.causal_claim_count,
            self.features.len()
        )
    }
}

/// The difference between two world states.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldDiff {
    pub from: Timestamp,
    pub to: Timestamp,
    pub changes: Vec<Change>,
}

impl WorldDiff {
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// Changes at or above a materiality threshold, most material first.
    pub fn material(&self, threshold: f64) -> Vec<&Change> {
        let mut out: Vec<&Change> = self
            .changes
            .iter()
            .filter(|c| c.materiality >= threshold)
            .collect();
        out.sort_by(|a, b| {
            b.materiality
                .partial_cmp(&a.materiality)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.subject.cmp(&b.subject))
        });
        out
    }

    pub fn of_kind(&self, kind: ChangeKind) -> Vec<&Change> {
        self.changes.iter().filter(|c| c.kind == kind).collect()
    }

    /// A readable account of what changed.
    pub fn summary(&self) -> String {
        if self.changes.is_empty() {
            return format!(
                "nothing changed between {} and {}",
                self.from.to_rfc3339(),
                self.to.to_rfc3339()
            );
        }
        let material = self.material(0.5);
        format!(
            "{} changes between {} and {}, {} of them material: {}",
            self.changes.len(),
            self.from.to_rfc3339(),
            self.to.to_rfc3339(),
            material.len(),
            material
                .iter()
                .take(5)
                .map(|c| c.description.clone())
                .collect::<Vec<_>>()
                .join("; ")
        )
    }
}
