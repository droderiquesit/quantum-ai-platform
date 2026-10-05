//! The decision logic of self-observation: one entity graph, anomaly
//! detection over the platform's own series, an incident timeline, runbook-only
//! remediation with an audit trail, and an error-budget release gate.
//!
//! Everything here is pure: no sockets, no clock reads, no environment. The
//! one effect a remediation can have is behind the [`Effector`] trait, which a
//! composition root implements; nothing in this module can reach a venue, so
//! the paper-trading boundary is not touched.

use crate::slo::SloStatus;
use qip_core::{Error, Result, Timestamp, sha256_hex};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

// --- OBS-001: one graph, five signal classes --------------------------------

/// The five classes of signal the platform observes itself through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalClass {
    Infrastructure,
    Application,
    DataQuality,
    ModelHealth,
    TradingBehaviour,
}

/// What a signal is about, e.g. `("instrument", "XNYS:ACME")`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EntityKey {
    pub kind: String,
    pub id: String,
}

impl EntityKey {
    pub fn new(kind: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            id: id.into(),
        }
    }
}

/// One observation of one class.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Signal {
    pub class: SignalClass,
    pub name: String,
    pub value: f64,
    pub at: Timestamp,
}

/// Signals keyed by the entity they concern. An entity shared by several
/// classes is one node: the join an operator would otherwise do by hand.
#[derive(Clone, Debug, Default)]
pub struct EntityGraph {
    nodes: BTreeMap<EntityKey, Vec<Signal>>,
}

impl EntityGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach `signal` to every entity it concerns.
    ///
    /// A signal about no entity is refused: it would be reachable from
    /// nowhere, which is the per-class silo this graph exists to remove.
    pub fn record(&mut self, signal: &Signal, entities: &[EntityKey]) -> Result<()> {
        if entities.is_empty() {
            return Err(Error::invalid(
                "a signal must name at least one entity it concerns; attach it to an instrument, deployment or model",
            ));
        }
        for entity in entities {
            if entity.kind.is_empty() || entity.id.is_empty() {
                return Err(Error::invalid(
                    "an entity key needs a non-empty kind and id",
                ));
            }
        }
        for entity in entities {
            self.nodes
                .entry(entity.clone())
                .or_default()
                .push(signal.clone());
        }
        Ok(())
    }

    pub fn signals_for(&self, entity: &EntityKey) -> &[Signal] {
        self.nodes.get(entity).map_or(&[], Vec::as_slice)
    }

    pub fn classes_for(&self, entity: &EntityKey) -> BTreeSet<SignalClass> {
        self.signals_for(entity).iter().map(|s| s.class).collect()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

// --- OBS-003: anomalies over the platform's own series ----------------------

/// A detected level shift in one metric series.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Anomaly {
    pub series: String,
    pub window_start: Timestamp,
    pub window_end: Timestamp,
    /// Shift in units of the series' own robust noise scale.
    pub score: f64,
}

/// Robust score a level shift must reach. Eight noise-scales is far outside
/// anything the window's own spread produces and needs no per-series threshold.
const SHIFT_SCORE: f64 = 8.0;

fn median(sorted: &mut [f64]) -> f64 {
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        f64::midpoint(sorted[n / 2 - 1], sorted[n / 2])
    }
}

fn median_and_mad(xs: &[f64]) -> (f64, f64) {
    let mut v = xs.to_vec();
    let m = median(&mut v);
    let mut dev: Vec<f64> = xs.iter().map(|x| (x - m).abs()).collect();
    (m, median(&mut dev))
}

/// Find the strongest level shift in `points` using `half` samples either side.
///
/// Threshold-free in the sense the requirement means: no absolute limit on the
/// metric is configured, so a shift crossing no static threshold is still
/// found. Fewer than `2 * half` points, or `half < 3`, is refused rather than
/// reported as "no anomaly" — a detector that cannot see is not a quiet one.
pub fn detect_level_shift(
    series: &str,
    points: &[(Timestamp, f64)],
    half: usize,
) -> Result<Option<Anomaly>> {
    if half < 3 || points.len() < 2 * half {
        return Err(Error::invalid(
            "level-shift detection needs half >= 3 and at least 2 * half samples; widen the series or shrink the window",
        ));
    }
    if points.iter().any(|(_, v)| !v.is_finite()) {
        return Err(Error::numeric(
            "a non-finite sample reached the anomaly detector; drop it at the source",
        ));
    }
    let values: Vec<f64> = points.iter().map(|p| p.1).collect();
    let mut best: Option<Anomaly> = None;
    for i in half..=values.len() - half {
        let (mb, madb) = median_and_mad(&values[i - half..i]);
        let (ma, mada) = median_and_mad(&values[i..i + half]);
        // A flat window has MAD 0; floor the scale at 0.1% of the level so a
        // constant series stepping by a hair is not scored as infinite.
        let scale = (1.4826 * madb.max(mada))
            .max(0.001 * mb.abs().max(ma.abs()))
            .max(1e-12);
        let score = (ma - mb).abs() / scale;
        if score >= SHIFT_SCORE && best.as_ref().is_none_or(|b| score > b.score) {
            best = Some(Anomaly {
                series: series.to_string(),
                window_start: points[i - half].0,
                window_end: points[i + half - 1].0,
                score,
            });
        }
    }
    Ok(best)
}

// --- OBS-004: one timeline per incident -------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineSource {
    Telemetry,
    Deployment,
    DataAnomaly,
    VenueBehaviour,
    ModelDrift,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub source: TimelineSource,
    pub at: Timestamp,
    pub detail: String,
}

impl From<&Anomaly> for TimelineEvent {
    fn from(a: &Anomaly) -> Self {
        Self {
            source: TimelineSource::Telemetry,
            at: a.window_start,
            detail: format!("level shift in {} (score {:.1})", a.series, a.score),
        }
    }
}

/// Events inside `[start, end]`, in timestamp order (ties by source), each
/// keeping the source it came from. An inverted window is refused.
pub fn build_timeline(
    start: Timestamp,
    end: Timestamp,
    events: &[TimelineEvent],
) -> Result<Vec<TimelineEvent>> {
    if end < start {
        return Err(Error::invalid(
            "incident window ends before it starts; pass start <= end",
        ));
    }
    let mut out: Vec<TimelineEvent> = events
        .iter()
        .filter(|e| e.at >= start && e.at <= end)
        .cloned()
        .collect();
    out.sort_by(|a, b| (a.at, a.source).cmp(&(b.at, b.source)));
    Ok(out)
}

// --- OBS-005 / OBS-030: runbook-only remediation, audited -------------------

/// A pre-approved runbook: one named action, on a fixed set of targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runbook {
    pub name: String,
    /// `restart`, `rollback`, `scale`, `failover`, `drain`, `quarantine` or
    /// `disable_capability`.
    pub action: String,
    /// Targets this runbook may touch. Scale lists permitted services only.
    pub targets: BTreeSet<String>,
}

/// The actions a runbook may name at all.
pub const PERMITTED_ACTIONS: &[&str] = &[
    "restart",
    "rollback",
    "scale",
    "failover",
    "drain",
    "quarantine",
    "disable_capability",
];

/// What carries out an approved action. Implemented by a composition root.
pub trait Effector {
    fn apply(&mut self, action: &str, target: &str) -> Result<String>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub seq: u64,
    pub incident: String,
    pub action: String,
    pub target: String,
    /// The runbook that authorised it; empty on a refusal.
    pub runbook: String,
    pub result: String,
    pub prev_hash: String,
    pub hash: String,
}

/// Append-only, hash-chained. There is deliberately no method that removes or
/// rewrites a record, and `append` is not public, so an agent holding a
/// `Remediator` can read its trail and nothing more.
#[derive(Clone, Debug, Default)]
pub struct AuditTrail {
    records: Vec<AuditRecord>,
}

impl AuditTrail {
    fn append(&mut self, incident: &str, action: &str, target: &str, runbook: &str, result: &str) {
        let prev_hash = self
            .records
            .last()
            .map_or_else(String::new, |r| r.hash.clone());
        let seq = self.records.len() as u64;
        let hash = sha256_hex(
            format!("{seq}|{incident}|{action}|{target}|{runbook}|{result}|{prev_hash}").as_bytes(),
        );
        self.records.push(AuditRecord {
            seq,
            incident: incident.into(),
            action: action.into(),
            target: target.into(),
            runbook: runbook.into(),
            result: result.into(),
            prev_hash,
            hash,
        });
    }

    pub fn records(&self) -> &[AuditRecord] {
        &self.records
    }

    /// Whether the chain still links; false if any record was altered.
    pub fn verify(&self) -> bool {
        let mut prev = String::new();
        self.records.iter().enumerate().all(|(i, r)| {
            let expect = sha256_hex(
                format!(
                    "{i}|{}|{}|{}|{}|{}|{prev}",
                    r.incident, r.action, r.target, r.runbook, r.result
                )
                .as_bytes(),
            );
            let ok = r.seq == i as u64 && r.prev_hash == prev && r.hash == expect;
            prev = r.hash.clone();
            ok
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_altered_audit_record_no_longer_verifies() {
        let mut trail = AuditTrail::default();
        trail.append("INC", "restart", "svc", "rb", "done");
        trail.append("INC", "drain", "svc", "rb2", "done");
        assert!(trail.verify(), "premise: an untouched chain verifies");
        trail.records[0].result = "nothing happened".into();
        assert!(!trail.verify());
    }
}

#[derive(Debug)]
pub struct Remediator<E: Effector + std::fmt::Debug> {
    runbooks: Vec<Runbook>,
    effector: E,
    audit: AuditTrail,
}

impl<E: Effector + std::fmt::Debug> Remediator<E> {
    /// Refuses a runbook naming an action outside [`PERMITTED_ACTIONS`], so
    /// "delete a topic" cannot be approved by registering it.
    pub fn new(runbooks: Vec<Runbook>, effector: E) -> Result<Self> {
        for r in &runbooks {
            if !PERMITTED_ACTIONS.contains(&r.action.as_str()) {
                return Err(Error::denied(format!(
                    "runbook {} names action {} which is not one of {PERMITTED_ACTIONS:?}",
                    r.name, r.action
                )));
            }
        }
        Ok(Self {
            runbooks,
            effector,
            audit: AuditTrail::default(),
        })
    }

    pub fn audit(&self) -> &AuditTrail {
        &self.audit
    }

    /// Execute `action` on `target` only if a runbook names both; otherwise
    /// record a refusal naming the missing runbook. Both outcomes are audited.
    pub fn remediate(&mut self, incident: &str, action: &str, target: &str) -> Result<String> {
        let found = self
            .runbooks
            .iter()
            .find(|r| r.action == action && r.targets.contains(target))
            .map(|r| r.name.clone());
        let Some(runbook) = found else {
            let why = format!("refused: no runbook names action {action} on {target}");
            self.audit.append(incident, action, target, "", &why);
            return Err(Error::denied(why));
        };
        match self.effector.apply(action, target) {
            Ok(result) => {
                self.audit
                    .append(incident, action, target, &runbook, &result);
                Ok(result)
            }
            Err(e) => {
                self.audit
                    .append(incident, action, target, &runbook, &format!("failed: {e}"));
                Err(e)
            }
        }
    }
}

// --- OBS-028: error budgets drive release -----------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseDecision {
    Proceed,
    Refused { reason: String },
}

/// Refuse a promotion to `plane` while any observed objective of that plane has
/// spent its whole error budget. An objective nothing measured never blocks:
/// it has no budget state to act on, and says so by being unobserved.
pub fn release_gate(plane: &str, statuses: &[SloStatus]) -> ReleaseDecision {
    for s in statuses.iter().filter(|s| s.slo.service == plane) {
        if s.is_observed() && s.budget_consumed >= 1.0 {
            return ReleaseDecision::Refused {
                reason: format!(
                    "error budget exhausted for {} ({:.2} consumed)",
                    s.slo.name, s.budget_consumed
                ),
            };
        }
    }
    ReleaseDecision::Proceed
}
