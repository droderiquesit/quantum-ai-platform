//! What datasets exist, where they came from, what they may be used for, and
//! whether they are currently trustworthy.
//!
//! The catalogue is not documentation. Three of its questions have answers a
//! caller must act on: `lineage_of` says whether a dataset's ancestry resolves
//! to roots or names the link that is missing, `usable_for` refuses a
//! quarantined dataset regardless of its licence, and `impacted_by` says what
//! else has to stop when one dataset is found to be wrong.
//!
//! Licensing metadata is [`qip_contracts::Entitlement`], the same type
//! `qip-compliance` enforces against, so the catalogue and the entitlement
//! control cannot disagree about what a licence says.

use crate::provider::MeshPort;
use qip_contracts::governance::{Entitlement, Usage};
use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// How much a dataset can currently be trusted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityState {
    /// Registered, never checked. Usable — a platform that refused everything
    /// unverified would refuse everything on its first day — but visible.
    Unverified,
    Verified {
        at: Timestamp,
        checks: Vec<String>,
    },
    /// Known to be imperfect but still usable, with the caveat recorded.
    Degraded {
        since: Timestamp,
        reason: String,
    },
    /// Not usable. Something is wrong that has not been characterised.
    Quarantined {
        since: Timestamp,
        reason: String,
    },
}

impl QualityState {
    /// Whether a dataset in this state may be read at all.
    ///
    /// Degraded is usable and quarantined is not, because the two exist to be
    /// different: collapsing them would mean either that every imperfection
    /// stops the platform or that nothing does.
    pub fn is_usable(&self) -> bool {
        !matches!(self, Self::Quarantined { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Unverified => "unverified".to_string(),
            Self::Verified { at, checks } => {
                format!("verified at {at} by {} check(s)", checks.len())
            }
            Self::Degraded { since, reason } => format!("degraded since {since}: {reason}"),
            Self::Quarantined { since, reason } => {
                format!("quarantined since {since}: {reason}")
            }
        }
    }
}

/// One dataset's registration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DatasetRegistration {
    pub dataset: String,
    /// The team accountable for it.
    pub owner: String,
    /// Which access pattern serves it.
    pub port: MeshPort,
    /// The datasets this one was produced from. Empty means a root.
    pub produced_from: Vec<String>,
    /// What it may be used for, in the same vocabulary `qip-compliance`
    /// enforces.
    pub entitlements: Vec<Entitlement>,
    pub quality: QualityState,
    pub registered_at: Timestamp,
}

impl DatasetRegistration {
    pub fn new(
        dataset: impl Into<String>,
        owner: impl Into<String>,
        port: MeshPort,
        registered_at: Timestamp,
    ) -> Result<Self> {
        let dataset = dataset.into();
        let owner = owner.into();
        if dataset.trim().is_empty() {
            return Err(Error::invalid("a dataset must be named"));
        }
        if owner.trim().is_empty() {
            return Err(Error::invalid(format!(
                "dataset {dataset} must name an accountable owner; an unowned dataset is one \
                 nobody fixes"
            )));
        }
        Ok(Self {
            dataset,
            owner,
            port,
            produced_from: Vec::new(),
            entitlements: Vec::new(),
            quality: QualityState::Unverified,
            registered_at,
        })
    }

    pub fn produced_from(mut self, parents: Vec<String>) -> Self {
        self.produced_from = parents;
        self
    }

    pub fn licensed(mut self, entitlement: Entitlement) -> Self {
        self.entitlements.push(entitlement);
        self
    }

    pub fn with_quality(mut self, quality: QualityState) -> Self {
        self.quality = quality;
        self
    }

    /// Whether this dataset is licensed for a usage at `now`.
    ///
    /// Bounded below by [`Self::registered_at`] as well as above by each
    /// entitlement's own expiry. Without the lower bound, a query about an
    /// instant before the dataset was ever registered into the mesh would
    /// answer exactly as it would for today — an entitlement granted after
    /// the fact would read as having covered a moment nobody could yet have
    /// used the dataset for, which is retroactive leakage in the licensing
    /// domain's own terms. `registered_at` is already recorded on every
    /// registration, so this is not an opt-in a caller can forget: nothing
    /// stops the mesh knowing about a dataset before the mesh knew about it.
    pub fn permits(&self, usage: Usage, now: Timestamp) -> bool {
        if now < self.registered_at {
            return false;
        }
        self.entitlements.iter().any(|e| match e {
            Entitlement::Granted { usage: u, .. } => *u == usage && e.is_granted(now),
            Entitlement::Denied { .. } => false,
        })
    }
}

/// A link in a lineage walk that does not resolve.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageBreak {
    /// The parent that is not registered.
    pub missing: String,
    /// The dataset that claims it as a parent.
    pub referenced_by: String,
}

impl LineageBreak {
    pub fn describe(&self) -> String {
        format!(
            "`{}` claims a parent `{}` that is not registered",
            self.referenced_by, self.missing
        )
    }
}

/// Where a dataset came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetLineage {
    dataset: String,
    /// Registered datasets with no parents of their own.
    roots: Vec<String>,
    /// Every resolved parent link reached, as `(child, parent)`.
    edges: Vec<(String, String)>,
    breaks: Vec<LineageBreak>,
    /// Datasets that are their own ancestor. A cycle in lineage means two
    /// pipelines each believe they feed the other, and neither can be rebuilt.
    cycles: Vec<String>,
    depth: usize,
}

impl DatasetLineage {
    pub fn dataset(&self) -> &str {
        &self.dataset
    }

    pub fn roots(&self) -> &[String] {
        &self.roots
    }

    pub fn edges(&self) -> &[(String, String)] {
        &self.edges
    }

    pub fn breaks(&self) -> &[LineageBreak] {
        &self.breaks
    }

    pub fn cycles(&self) -> &[String] {
        &self.cycles
    }

    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Whether the walk resolved to roots with nothing missing.
    pub fn is_resolved(&self) -> bool {
        self.breaks.is_empty() && self.cycles.is_empty()
    }

    /// The lineage, or an error naming exactly what does not resolve.
    pub fn require_resolved(&self) -> Result<()> {
        if !self.cycles.is_empty() {
            return Err(Error::invalid(format!(
                "the lineage of `{}` is cyclical through: {}",
                self.dataset,
                self.cycles.join(", ")
            )));
        }
        if !self.breaks.is_empty() {
            let detail: Vec<String> = self.breaks.iter().map(LineageBreak::describe).collect();
            return Err(Error::not_found(format!(
                "the lineage of `{}` does not resolve: {}",
                self.dataset,
                detail.join("; ")
            )));
        }
        Ok(())
    }
}

/// Every dataset the mesh knows about.
#[derive(Debug, Default)]
pub struct Catalog {
    entries: BTreeMap<String, DatasetRegistration>,
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register or replace a dataset's entry.
    ///
    /// A dataset naming itself as a parent is refused here rather than left
    /// for the lineage walk: it is always a typo, and catching it at
    /// registration means the walk's cycle report is about real cycles.
    pub fn register(&mut self, registration: DatasetRegistration) -> Result<()> {
        if registration.produced_from.contains(&registration.dataset) {
            return Err(Error::invalid(format!(
                "dataset `{}` lists itself as its own parent",
                registration.dataset
            )));
        }
        self.entries
            .insert(registration.dataset.clone(), registration);
        Ok(())
    }

    pub fn get(&self, dataset: &str) -> Option<&DatasetRegistration> {
        self.entries.get(dataset)
    }

    pub fn iter(&self) -> impl Iterator<Item = &DatasetRegistration> {
        self.entries.values()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn require(&self, dataset: &str) -> Result<&DatasetRegistration> {
        self.entries
            .get(dataset)
            .ok_or_else(|| Error::not_found(format!("no dataset `{dataset}` is registered")))
    }

    /// Record a quality finding.
    pub fn set_quality(&mut self, dataset: &str, quality: QualityState) -> Result<()> {
        let entry = self
            .entries
            .get_mut(dataset)
            .ok_or_else(|| Error::not_found(format!("no dataset `{dataset}` is registered")))?;
        entry.quality = quality;
        Ok(())
    }

    /// Stop a dataset being used, with a stated reason.
    pub fn quarantine(
        &mut self,
        dataset: &str,
        reason: impl Into<String>,
        at: Timestamp,
    ) -> Result<()> {
        let reason = reason.into();
        if reason.trim().len() < 10 {
            return Err(Error::invalid(
                "quarantining a dataset needs a reason somebody can act on",
            ));
        }
        self.set_quality(dataset, QualityState::Quarantined { since: at, reason })
    }

    /// Whether a dataset may be read for a purpose right now.
    ///
    /// Quality is checked before licensing. A quarantined dataset is refused
    /// however well licensed it is: the licence says what the platform is
    /// permitted to do with correct data, not with data known to be wrong.
    pub fn usable_for(&self, dataset: &str, usage: Usage, now: Timestamp) -> Result<()> {
        let entry = self.require(dataset)?;
        if !entry.quality.is_usable() {
            return Err(Error::guard(format!(
                "dataset `{dataset}` is {} and may not be read",
                entry.quality.describe()
            )));
        }
        if now < entry.registered_at {
            return Err(Error::denied(format!(
                "dataset `{dataset}` was not registered until {}, and {now} is before that; \
                 a registration made today cannot retroactively cover an instant before it \
                 existed",
                entry.registered_at
            )));
        }
        if !entry.permits(usage, now) {
            return Err(Error::denied(format!(
                "dataset `{dataset}` is not licensed for {} at {now}",
                usage.as_str()
            )));
        }
        Ok(())
    }

    /// Walk a dataset back to the datasets it was produced from.
    ///
    /// Breadth-first with a visited set, so a diamond is walked once and a
    /// cycle is reported rather than followed forever.
    pub fn lineage_of(&self, dataset: &str) -> Result<DatasetLineage> {
        self.require(dataset)?;
        let mut roots = BTreeSet::new();
        let mut edges = Vec::new();
        let mut breaks = Vec::new();
        let mut cycles = BTreeSet::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut depth = 0usize;
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();

        seen.insert(dataset.to_string());
        queue.push_back((dataset.to_string(), 0));

        while let Some((current, at_depth)) = queue.pop_front() {
            depth = depth.max(at_depth);
            let Some(entry) = self.entries.get(&current) else {
                continue;
            };
            if entry.produced_from.is_empty() {
                roots.insert(current.clone());
                continue;
            }
            for parent in &entry.produced_from {
                edges.push((current.clone(), parent.clone()));
                if !self.entries.contains_key(parent) {
                    breaks.push(LineageBreak {
                        missing: parent.clone(),
                        referenced_by: current.clone(),
                    });
                    continue;
                }
                if parent == dataset {
                    cycles.insert(parent.clone());
                    continue;
                }
                if seen.insert(parent.clone()) {
                    queue.push_back((parent.clone(), at_depth + 1));
                }
            }
        }

        Ok(DatasetLineage {
            dataset: dataset.to_string(),
            roots: roots.into_iter().collect(),
            edges,
            breaks,
            cycles: cycles.into_iter().collect(),
            depth,
        })
    }

    /// Datasets that were produced from this one, directly.
    pub fn children_of(&self, dataset: &str) -> Vec<&str> {
        self.entries
            .values()
            .filter(|e| e.produced_from.iter().any(|p| p == dataset))
            .map(|e| e.dataset.as_str())
            .collect()
    }

    /// Everything downstream of a dataset, transitively.
    ///
    /// What an incident actually needs: when a feed is found to be wrong, this
    /// is the list of things computed from it that are now also suspect.
    pub fn impacted_by(&self, dataset: &str) -> Vec<String> {
        let mut impacted = BTreeSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();
        queue.push_back(dataset.to_string());
        while let Some(current) = queue.pop_front() {
            for child in self.children_of(&current) {
                if impacted.insert(child.to_string()) {
                    queue.push_back(child.to_string());
                }
            }
        }
        impacted.into_iter().collect()
    }

    /// Registered datasets whose lineage does not resolve, with the reason.
    ///
    /// Surfaced on an operator dashboard: a dataset whose ancestry quietly
    /// stopped resolving is a rebuild that will fail at the worst moment.
    pub fn unresolved(&self) -> Vec<(String, String)> {
        self.entries
            .keys()
            .filter_map(|dataset| {
                self.lineage_of(dataset).ok().and_then(|lineage| {
                    lineage
                        .require_resolved()
                        .err()
                        .map(|error| (dataset.clone(), error.message().to_string()))
                })
            })
            .collect()
    }
}
