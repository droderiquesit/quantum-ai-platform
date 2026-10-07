//! The World Model Federation: a population of declared, scored, expiring
//! world models, branches that keep their lineage, and an arbitration that
//! records disagreement instead of averaging it away (blueprint §9.3).
//!
//! # Why this exists
//!
//! One shared `WorldModel` that every consumer reads cannot say "the models
//! disagree", because there is no second model. An average of two views hides
//! that the platform does not know. So the federation keeps every member's
//! view, a measure of how far they diverge, and a recorded reason for what
//! arbitration did about it. An arbitrated probability exists only inside an
//! [`Arbitration`], which always carries the [`Disagreement`] it came from:
//! there is no function that returns the merged number alone.
//!
//! # What is held and what is not
//!
//! This is the in-process model: a registry, a lineage tree, calibration
//! scores and a journal of [`Event`]s from which the lineage tree can be
//! rebuilt. It is not yet appended to the central event log and no
//! composition root builds a `Federation`; the register says so.

use std::collections::{BTreeMap, BTreeSet};

use qip_contracts::expansion::EffectAttribution;
use qip_core::error::{Error, Result};
use qip_core::time::{Duration, Timestamp};

/// Spread (max minus min probability) at or under which views agree.
pub const AGREEMENT_SPREAD: f64 = 0.05;
/// Mean reported uncertainty at or over which arbitration abstains.
pub const ABSTAIN_UNCERTAINTY: f64 = 0.8;
/// Composite-score lead the best view needs over the runner-up for the
/// weighted resolution to be preferred to keeping both branches live.
pub const CLEAR_LEADER_MARGIN: f64 = 0.1;
/// Spread at or over which a disagreement is material: worth a research task
/// rather than only a journal line (WORLD-061). Four times
/// [`AGREEMENT_SPREAD`], so the band between the two is disagreement that is
/// recorded and not yet worth anyone's attention.
pub const MATERIAL_DISAGREEMENT_SPREAD: f64 = 0.2;

/// What a model is for, beyond what it is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelKind {
    Observational,
    Scenario,
    Counterfactual,
}

/// How far from raw observations a model reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Abstraction {
    Micro,
    Meso,
    Macro,
}

/// The five axes along which the population is specialised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub domain: String,
    /// A region, or the explicit word `global`; never blank.
    pub region: String,
    pub horizon: Duration,
    pub abstraction: Abstraction,
    pub hypothesis: String,
    pub kind: ModelKind,
}

/// The calibration a model claims for itself: the Brier score it says it will
/// stay at or under. Declared so that a score can later be read against it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalibrationClaim {
    pub max_brier: f64,
}

/// An un-validated declaration. Every field is optional on purpose, so a
/// missing one is a refusal that names it rather than a type error nobody
/// reads.
#[derive(Debug, Clone, Default)]
pub struct Declaration {
    pub id: String,
    pub scope: Option<Scope>,
    pub evidence_lineage: Vec<String>,
    pub calibration: Option<CalibrationClaim>,
    pub update_cadence: Option<Duration>,
    pub expires_at: Option<Timestamp>,
}

/// A validated declaration. Only [`WorldModelSpec::declare`] makes one.
#[derive(Debug, Clone)]
pub struct WorldModelSpec {
    id: String,
    scope: Scope,
    evidence_lineage: Vec<String>,
    calibration: CalibrationClaim,
    update_cadence: Duration,
    expires_at: Timestamp,
}

fn missing(id: &str, field: &str) -> Error {
    Error::invalid(format!(
        "world model `{id}` declares no {field}; declare scope, evidence lineage, calibration, \
         update cadence and expiry, or do not register the model"
    ))
}

impl WorldModelSpec {
    pub fn declare(d: Declaration) -> Result<Self> {
        if d.id.trim().is_empty() {
            return Err(Error::invalid("a world model needs a non-blank id"));
        }
        let id = d.id.as_str();
        let scope = d.scope.clone().ok_or_else(|| missing(id, "scope"))?;
        if scope.domain.trim().is_empty()
            || scope.region.trim().is_empty()
            || scope.hypothesis.trim().is_empty()
            || scope.horizon <= Duration::default()
        {
            return Err(Error::invalid(format!(
                "world model `{id}` has a blank or non-positive scope axis; name the domain, \
                 region (or `global`), horizon and hypothesis"
            )));
        }
        if d.evidence_lineage.is_empty() || d.evidence_lineage.iter().any(|e| e.trim().is_empty()) {
            return Err(missing(id, "evidence lineage"));
        }
        let calibration = d.calibration.ok_or_else(|| missing(id, "calibration"))?;
        if !(calibration.max_brier.is_finite()
            && calibration.max_brier > 0.0
            && calibration.max_brier <= 1.0)
        {
            return Err(Error::invalid(format!(
                "world model `{id}` claims a calibration outside (0, 1]; a Brier ceiling is a \
                 probability-scale number"
            )));
        }
        let update_cadence = d
            .update_cadence
            .ok_or_else(|| missing(id, "update cadence"))?;
        if update_cadence <= Duration::default() {
            return Err(missing(id, "positive update cadence"));
        }
        let expires_at = d.expires_at.ok_or_else(|| missing(id, "expiry"))?;
        Ok(Self {
            id: d.id,
            scope,
            evidence_lineage: d.evidence_lineage,
            calibration,
            update_cadence,
            expires_at,
        })
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn evidence_lineage(&self) -> &[String] {
        &self.evidence_lineage
    }
    pub fn update_cadence(&self) -> Duration {
        self.update_cadence
    }
    pub fn expires_at(&self) -> Timestamp {
        self.expires_at
    }
}

/// Where a node came from. A root model has no parents and no trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lineage {
    pub parents: Vec<String>,
    pub trigger: Option<String>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Live,
    /// Refuted by an outcome. Kept, with the Brier score of the forecast that
    /// was wrong, so the record stays readable.
    Closed {
        at: Timestamp,
        proposition: String,
        score: f64,
    },
    Merged {
        into: String,
        at: Timestamp,
    },
}

/// One model's view on one proposition, with the three dimensions it can
/// self-report. The fourth, predictive performance, is computed, never
/// reported.
#[derive(Debug, Clone, PartialEq)]
pub struct Assessment {
    pub proposition: String,
    pub probability: f64,
    pub evidence_fit: f64,
    pub causal_coherence: f64,
    pub uncertainty: f64,
    pub at: Timestamp,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Calibration {
    /// No forecast has resolved. Not zero, not a default.
    Unscored,
    Brier {
        score: f64,
        resolved: usize,
        within_claim: bool,
    },
}

#[derive(Debug)]
struct Node {
    spec: WorldModelSpec,
    lineage: Lineage,
    status: Status,
    views: BTreeMap<String, Assessment>,
    last_state_at: Option<Timestamp>,
    briers: Vec<f64>,
    expiry_recorded: bool,
}

/// One live member's standing on all four dimensions.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredView {
    pub model: String,
    pub probability: f64,
    pub evidence_fit: f64,
    /// `1 - Brier`, or `None` while the model is unscored.
    pub predictive_performance: Option<f64>,
    pub causal_coherence: f64,
    pub uncertainty: f64,
    /// Mean of the four, with uncertainty counted as `1 - uncertainty`.
    pub composite: Option<f64>,
}

/// Every live view, minority included, and how far apart they are.
#[derive(Debug, Clone, PartialEq)]
pub struct Disagreement {
    pub views: Vec<ScoredView>,
    pub spread: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InformationRequest {
    pub proposition: String,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Branch,
    Merge,
    Abstain,
    RequestMoreInformation(InformationRequest),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Arbitration {
    pub proposition: String,
    pub at: Timestamp,
    pub decision: Decision,
    pub reason: String,
    pub disagreement: Disagreement,
    /// Present only for `Merge`. Never returned apart from `disagreement`.
    pub arbitrated_probability: Option<f64>,
    /// Models left out because their expiry had passed.
    pub expired: Vec<String>,
    /// The research task this arbitration raised, when the disagreement was
    /// material and no task on the proposition was already open.
    pub research: Option<ResearchTask>,
}

/// A question the models' disagreement put to research (WORLD-061).
///
/// A recorded disagreement nobody is asked to resolve is a journal line. The
/// task is what turns "the models are far apart on this" into work, and it
/// cites the record it came from rather than restating it, so a reader of
/// the task reads the same views arbitration saw.
#[derive(Debug, Clone, PartialEq)]
pub struct ResearchTask {
    /// Position in the journal of the [`Event::Decided`] record cited.
    pub cites: usize,
    pub proposition: String,
    pub spread: f64,
    /// Every model whose view is in the cited record.
    pub models: Vec<String>,
    pub raised_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Declared {
        id: String,
        at: Timestamp,
    },
    Branched {
        id: String,
        lineage: Lineage,
    },
    Merged {
        id: String,
        lineage: Lineage,
    },
    Expired {
        id: String,
        at: Timestamp,
        expired_at: Timestamp,
    },
    Renewed {
        id: String,
        expires_at: Timestamp,
        at: Timestamp,
    },
    Closed {
        id: String,
        at: Timestamp,
        score: f64,
    },
    /// One arbitration, with the disagreement it found: the disagreement
    /// record a [`ResearchTask`] cites.
    Decided {
        proposition: String,
        at: Timestamp,
        reason: String,
        disagreement: Disagreement,
    },
    ResearchRaised(ResearchTask),
    /// AGENCY-002: an effect attribution applied to update the federation.
    /// Records that an EffectAttribution updated a world model, causal edge
    /// and/or action policy, with the citation to the attribution's ID.
    AttributionApplied {
        /// ID of the EffectAttribution that caused this update.
        attribution_id: String,
        /// The model the attribution updated.
        model_id: String,
        /// A text summary of what was updated (e.g., "causal_edge", "action_policy").
        update_kind: String,
        at: Timestamp,
    },
}

/// The distinct values the live population takes on each axis.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spanned {
    pub domains: BTreeSet<String>,
    pub regions: BTreeSet<String>,
    pub horizons: BTreeSet<Duration>,
    pub abstractions: BTreeSet<Abstraction>,
    pub hypotheses: BTreeSet<String>,
    pub kinds: BTreeSet<ModelKind>,
}

#[derive(Debug, Default)]
pub struct Federation {
    nodes: BTreeMap<String, Node>,
    journal: Vec<Event>,
    /// Propositions with a research task outstanding. One task per open
    /// question: a disagreement that persists is arbitrated every cycle, and
    /// raising it every cycle would bury the task under copies of itself.
    /// Cleared when reality answers the proposition.
    open_research: BTreeSet<String>,
}

fn unit(name: &str, v: f64) -> Result<()> {
    if v.is_finite() && (0.0..=1.0).contains(&v) {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "{name} must lie in [0, 1], got {v}; a value is refused rather than clamped"
        )))
    }
}

impl Federation {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn journal(&self) -> &[Event] {
        &self.journal
    }

    /// Every research task raised, oldest first, read from the journal.
    pub fn research_tasks(&self) -> Vec<&ResearchTask> {
        self.journal
            .iter()
            .filter_map(|e| match e {
                Event::ResearchRaised(task) => Some(task),
                _ => None,
            })
            .collect()
    }

    fn node(&self, id: &str) -> Result<&Node> {
        self.nodes
            .get(id)
            .ok_or_else(|| Error::not_found(format!("no world model or branch `{id}`")))
    }

    fn live_node_mut(&mut self, id: &str) -> Result<&mut Node> {
        let node = self
            .nodes
            .get_mut(id)
            .ok_or_else(|| Error::not_found(format!("no world model or branch `{id}`")))?;
        if node.status != Status::Live {
            return Err(Error::denied(format!(
                "`{id}` is closed or merged and is no longer updated; its record stays readable"
            )));
        }
        Ok(node)
    }

    fn insert(&mut self, spec: WorldModelSpec, lineage: Lineage) -> Result<()> {
        if self.nodes.contains_key(spec.id()) {
            return Err(Error::invalid(format!(
                "`{}` is already registered; ids are never reused",
                spec.id()
            )));
        }
        self.nodes.insert(
            spec.id().to_string(),
            Node {
                spec,
                lineage,
                status: Status::Live,
                views: BTreeMap::new(),
                last_state_at: None,
                briers: Vec::new(),
                expiry_recorded: false,
            },
        );
        Ok(())
    }

    pub fn register(&mut self, spec: WorldModelSpec, at: Timestamp) -> Result<()> {
        let id = spec.id().to_string();
        self.insert(
            spec,
            Lineage {
                parents: Vec::new(),
                trigger: None,
                created_at: at,
            },
        )?;
        self.journal.push(Event::Declared { id, at });
        Ok(())
    }

    pub fn lineage(&self, id: &str) -> Result<&Lineage> {
        Ok(&self.node(id)?.lineage)
    }

    pub fn status(&self, id: &str) -> Result<&Status> {
        Ok(&self.node(id)?.status)
    }

    /// A model states its view. Refused for a closed or merged node.
    pub fn assess(&mut self, id: &str, a: Assessment) -> Result<()> {
        unit("probability", a.probability)?;
        unit("evidence_fit", a.evidence_fit)?;
        unit("causal_coherence", a.causal_coherence)?;
        unit("uncertainty", a.uncertainty)?;
        if a.proposition.trim().is_empty() {
            return Err(Error::invalid("an assessment names its proposition"));
        }
        let node = self.live_node_mut(id)?;
        node.last_state_at = Some(a.at);
        node.views.insert(a.proposition.clone(), a);
        Ok(())
    }

    /// Branch a live node into a competing hypothesis. The branch inherits
    /// the parent's declaration with its own id and hypothesis.
    pub fn branch(
        &mut self,
        parent: &str,
        id: &str,
        hypothesis: &str,
        trigger: &str,
        at: Timestamp,
    ) -> Result<()> {
        if trigger.trim().is_empty() || hypothesis.trim().is_empty() {
            return Err(Error::invalid(
                "a branch names the evidence or event that triggered it and its hypothesis",
            ));
        }
        let mut spec = self.live_node_mut(parent)?.spec.clone();
        spec.id = id.to_string();
        spec.scope.hypothesis = hypothesis.to_string();
        let lineage = Lineage {
            parents: vec![parent.to_string()],
            trigger: Some(trigger.to_string()),
            created_at: at,
        };
        self.insert(spec, lineage.clone())?;
        self.journal.push(Event::Branched {
            id: id.to_string(),
            lineage,
        });
        Ok(())
    }

    /// Merge two live nodes. Both stay readable, marked merged, and the
    /// result names both as parents.
    pub fn merge(&mut self, a: &str, b: &str, id: &str, reason: &str, at: Timestamp) -> Result<()> {
        if a == b {
            return Err(Error::invalid("a merge needs two distinct parents"));
        }
        if reason.trim().is_empty() {
            return Err(Error::invalid("a merge records why it happened"));
        }
        self.live_node_mut(b)?;
        let mut spec = self.live_node_mut(a)?.spec.clone();
        spec.id = id.to_string();
        spec.scope.hypothesis = format!("{a}+{b}");
        let lineage = Lineage {
            parents: vec![a.to_string(), b.to_string()],
            trigger: Some(reason.to_string()),
            created_at: at,
        };
        self.insert(spec, lineage.clone())?;
        for parent in [a, b] {
            self.live_node_mut(parent)?.status = Status::Merged {
                into: id.to_string(),
                at,
            };
        }
        self.journal.push(Event::Merged {
            id: id.to_string(),
            lineage,
        });
        Ok(())
    }

    /// Extend an expiry. Refused unless the new instant is after `at`, since
    /// a renewal into the past would leave the model expired and say it was
    /// not.
    pub fn renew(&mut self, id: &str, expires_at: Timestamp, at: Timestamp) -> Result<()> {
        if expires_at <= at {
            return Err(Error::invalid(
                "a renewal must expire after the instant it is made",
            ));
        }
        let node = self.live_node_mut(id)?;
        node.spec.expires_at = expires_at;
        node.expiry_recorded = false;
        self.journal.push(Event::Renewed {
            id: id.to_string(),
            expires_at,
            at,
        });
        Ok(())
    }

    /// Reality answers `proposition`. Every holder of a forecast on it is
    /// scored; a live branch whose forecast was on the wrong side is closed
    /// with that score recorded, never deleted. Returns the ids closed.
    pub fn resolve(&mut self, proposition: &str, outcome: bool, at: Timestamp) -> Vec<String> {
        // Answered, so the question research was asked is closed; a later
        // disagreement on a proposition of the same name is a new question.
        self.open_research.remove(proposition);
        let truth = if outcome { 1.0 } else { 0.0 };
        let mut closed = Vec::new();
        for (id, node) in &mut self.nodes {
            let Some(view) = node.views.remove(proposition) else {
                continue;
            };
            let brier = (view.probability - truth).powi(2);
            node.briers.push(brier);
            let wrong_side = (view.probability - 0.5) * (truth - 0.5) < 0.0;
            if wrong_side && node.status == Status::Live && !node.lineage.parents.is_empty() {
                node.status = Status::Closed {
                    at,
                    proposition: proposition.to_string(),
                    score: brier,
                };
                closed.push(id.clone());
            }
        }
        for id in &closed {
            let score = self.nodes.get(id).and_then(|n| n.briers.last().copied());
            self.journal.push(Event::Closed {
                id: id.clone(),
                at,
                score: score.unwrap_or(1.0),
            });
        }
        closed
    }

    pub fn calibration(&self, id: &str) -> Result<Calibration> {
        let node = self.node(id)?;
        if node.briers.is_empty() {
            return Ok(Calibration::Unscored);
        }
        let score = node.briers.iter().sum::<f64>() / node.briers.len() as f64;
        Ok(Calibration::Brier {
            score,
            resolved: node.briers.len(),
            within_claim: score <= node.spec.calibration.max_brier,
        })
    }

    /// The distinct values live models take on each axis.
    pub fn spanned(&self) -> Spanned {
        let mut s = Spanned::default();
        for node in self.nodes.values().filter(|n| n.status == Status::Live) {
            let sc = &node.spec.scope;
            s.domains.insert(sc.domain.clone());
            s.regions.insert(sc.region.clone());
            s.horizons.insert(sc.horizon);
            s.abstractions.insert(sc.abstraction);
            s.hypotheses.insert(sc.hypothesis.clone());
            s.kinds.insert(sc.kind);
        }
        s
    }

    /// Live models that produced no state at or after `since`.
    pub fn silent_since(&self, since: Timestamp) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|(_, n)| n.status == Status::Live)
            .filter(|(_, n)| n.last_state_at.is_none_or(|t| t < since))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Compare every live, unexpired view on `proposition`.
    ///
    /// A model whose expiry is at or before `now` is excluded and the
    /// exclusion is journaled once as an expiry; renewing it brings it back.
    pub fn arbitrate(&mut self, proposition: &str, now: Timestamp) -> Result<Arbitration> {
        let mut expired = Vec::new();
        let mut views = Vec::new();
        for (id, node) in &mut self.nodes {
            if node.status != Status::Live {
                continue;
            }
            if now >= node.spec.expires_at {
                expired.push(id.clone());
                if !node.expiry_recorded {
                    node.expiry_recorded = true;
                    self.journal.push(Event::Expired {
                        id: id.clone(),
                        at: now,
                        expired_at: node.spec.expires_at,
                    });
                }
                continue;
            }
            let Some(a) = node.views.get(proposition) else {
                continue;
            };
            let performance = if node.briers.is_empty() {
                None
            } else {
                Some(1.0 - node.briers.iter().sum::<f64>() / node.briers.len() as f64)
            };
            views.push(ScoredView {
                model: id.clone(),
                probability: a.probability,
                evidence_fit: a.evidence_fit,
                predictive_performance: performance,
                causal_coherence: a.causal_coherence,
                uncertainty: a.uncertainty,
                composite: performance.map(|p| {
                    (a.evidence_fit + p + a.causal_coherence + (1.0 - a.uncertainty)) / 4.0
                }),
            });
        }
        let (lo, hi) = views
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
                (lo.min(v.probability), hi.max(v.probability))
            });
        let spread = if views.is_empty() { 0.0 } else { hi - lo };
        let (decision, reason, arbitrated) = decide(proposition, &views, spread);
        let disagreement = Disagreement { views, spread };
        let record = self.journal.len();
        self.journal.push(Event::Decided {
            proposition: proposition.to_string(),
            at: now,
            reason: reason.clone(),
            disagreement: disagreement.clone(),
        });
        // A material disagreement becomes a research task citing the record
        // just written, once per open proposition (WORLD-061). Raised on the
        // spread alone, whatever arbitration decided to do about it: a merge
        // behind a clear leader still left two models far apart, and that
        // is the thing worth looking into.
        let research = (spread >= MATERIAL_DISAGREEMENT_SPREAD
            && self.open_research.insert(proposition.to_string()))
        .then(|| ResearchTask {
            cites: record,
            proposition: proposition.to_string(),
            spread,
            models: disagreement.views.iter().map(|v| v.model.clone()).collect(),
            raised_at: now,
        });
        if let Some(task) = &research {
            self.journal.push(Event::ResearchRaised(task.clone()));
        }
        Ok(Arbitration {
            proposition: proposition.to_string(),
            at: now,
            decision,
            reason,
            disagreement,
            arbitrated_probability: arbitrated,
            expired,
            research,
        })
    }

    /// AGENCY-002: Apply an effect attribution to the federation. Updates the
    /// named world model and journals the update with citation to the
    /// attribution ID. The effect's mean and standard deviation are recorded as
    /// evidence the model can use; the identifiability marker tells whether the
    /// effect is causal or merely correlational.
    ///
    /// Returns the position in the journal of the update record.
    pub fn update_from_attribution(
        &mut self,
        attribution: &EffectAttribution,
        at: Timestamp,
    ) -> Result<usize> {
        if attribution.goal_id.trim().is_empty() {
            return Err(Error::invalid(
                "an effect attribution names the goal it resulted from",
            ));
        }
        let attribution_id = format!(
            "attr-{}-{}-{}",
            attribution.goal_id, attribution.intervention_id, attribution.action_id
        );
        let update_kind = format!(
            "effect_mean_{:.2}_std_{:.2}",
            attribution.effect.mean, attribution.effect.std_dev
        );
        let record_pos = self.journal.len();
        self.journal.push(Event::AttributionApplied {
            attribution_id,
            model_id: attribution.goal_id.clone(),
            update_kind,
            at,
        });
        Ok(record_pos)
    }
}

fn decide(proposition: &str, views: &[ScoredView], spread: f64) -> (Decision, String, Option<f64>) {
    let ask = |models: Vec<String>, why: String| {
        (
            Decision::RequestMoreInformation(InformationRequest {
                proposition: proposition.to_string(),
                models,
            }),
            why,
            None,
        )
    };
    if views.len() < 2 {
        return ask(
            views.iter().map(|v| v.model.clone()).collect(),
            format!("{} live view(s); two are needed to compare", views.len()),
        );
    }
    let unscored: Vec<String> = views
        .iter()
        .filter(|v| v.composite.is_none())
        .map(|v| v.model.clone())
        .collect();
    if !unscored.is_empty() {
        return ask(
            unscored,
            "views cannot be weighed while a model has no resolved forecast".to_string(),
        );
    }
    let mean_uncertainty = views.iter().map(|v| v.uncertainty).sum::<f64>() / views.len() as f64;
    if mean_uncertainty >= ABSTAIN_UNCERTAINTY {
        return (
            Decision::Abstain,
            format!("mean uncertainty {mean_uncertainty:.2} is too high to say anything"),
            None,
        );
    }
    let mut scores: Vec<f64> = views.iter().filter_map(|v| v.composite).collect();
    scores.sort_by(|a, b| b.total_cmp(a));
    let margin = scores[0] - scores[1];
    let agree = spread <= AGREEMENT_SPREAD;
    if agree || margin >= CLEAR_LEADER_MARGIN {
        let total: f64 = scores.iter().sum();
        if total <= 0.0 {
            return (
                Decision::Abstain,
                "every composite score is zero; there is nothing to weight by".to_string(),
                None,
            );
        }
        let weighted = views
            .iter()
            .map(|v| v.probability * v.composite.unwrap_or(0.0))
            .sum::<f64>()
            / total;
        let why = if agree {
            format!("views agree within {spread:.3}")
        } else {
            format!("views diverge by {spread:.3} but the leader is ahead by {margin:.3}")
        };
        return (Decision::Merge, why, Some(weighted));
    }
    (
        Decision::Branch,
        format!(
            "views diverge by {spread:.3} and the top two are within {margin:.3}; keep both live"
        ),
        None,
    )
}

/// Rebuild the lineage tree from a journal alone.
pub fn lineage_from_journal(journal: &[Event]) -> BTreeMap<String, Lineage> {
    let mut tree = BTreeMap::new();
    for e in journal {
        match e {
            Event::Declared { id, at } => {
                tree.insert(
                    id.clone(),
                    Lineage {
                        parents: Vec::new(),
                        trigger: None,
                        created_at: *at,
                    },
                );
            }
            Event::Branched { id, lineage } | Event::Merged { id, lineage } => {
                tree.insert(id.clone(), lineage.clone());
            }
            _ => {}
        }
    }
    tree
}
