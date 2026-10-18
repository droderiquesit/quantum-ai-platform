//! Step 1 of the expansion loop over measured state: the detectors that turn
//! what the platform recorded into [`Observation`]s (EXPAND-011, EXPAND-046,
//! EXPAND-064 to EXPAND-067).
//!
//! Until these existed `gap.rs` could classify an observation and nothing in
//! the tree ever produced one, so the six kinds of ignorance §22 names were
//! enumerated and never noticed. Each detector here reads a plain-data
//! reading the composition root fills from the platform — calibration hit
//! rates, declined orders scored afterwards, feasibility refusals, fill
//! residuals, the universe against the instruments that have history — and
//! states what it saw, the trigger, what is wanting, and the evidence. None
//! of them invents a severity: each severity is a ratio of two counts or a
//! residual against its own threshold, and the formula sits beside it.
//!
//! Pure functions and one small tracker. Nothing here reads a clock, a
//! socket or the environment, so a sweep replays from the readings alone.

use crate::gap::{GapTrigger, Observation, Wanting};
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use std::collections::{BTreeMap, BTreeSet};

/// Which of the detectors in this module raised an observation. Carried as
/// the first evidence id of every observation a detector makes, so a queued
/// item names its origin (EXPAND-064..069's "naming its origin").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Origin {
    /// A belief class whose resolved hit rate is below the floor.
    LowConfidence,
    /// An agent whose resolved hit rate is below the floor: no competent
    /// specialist holds that seat.
    IncompetentSpecialist,
    /// A gate whose declines, scored afterwards, repeatedly would have earned.
    RepeatedVeto,
    /// A venue refusing the same constraint repeatedly.
    RefusalCluster,
    /// A twin's fill prediction missing reality by more than its threshold.
    UnexplainedResidual,
    /// Two detectors disagreeing about the direction of the same subject.
    Contradiction,
    /// An asset class whose instruments have little or no history.
    Coverage,
    /// Several subjects showing the same unexplained symptom at once.
    AnomalyCluster,
    /// A discrepancy that persisted for several consecutive sweeps.
    PersistentDiscrepancy,
    /// A production model whose inputs drifted past its own threshold.
    ModelDrift,
    /// A hosted model refusing or garbling its replies past a stated rate.
    ModelErrorRate,
}

impl Origin {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LowConfidence => "detector:low_confidence",
            Self::IncompetentSpecialist => "detector:incompetent_specialist",
            Self::RepeatedVeto => "detector:repeated_veto",
            Self::RefusalCluster => "detector:refusal_cluster",
            Self::UnexplainedResidual => "detector:unexplained_residual",
            Self::Contradiction => "detector:contradiction",
            Self::Coverage => "detector:coverage",
            Self::AnomalyCluster => "detector:anomaly_cluster",
            Self::PersistentDiscrepancy => "detector:persistent_discrepancy",
            Self::ModelDrift => "detector:model_drift",
            Self::ModelErrorRate => "detector:model_error_rate",
        }
    }
}

fn fraction(name: &str, value: f64) -> Result<f64> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(Error::invalid(format!(
            "{name} must be a fraction in [0, 1], not {value}; a detector threshold outside it fires on everything or nothing"
        )));
    }
    Ok(value)
}

/// One observation with a single domain and its origin as first evidence.
fn observe(
    origin: Origin,
    trigger: GapTrigger,
    wanting: Wanting,
    observation: String,
    domain: &str,
    mut evidence: Vec<String>,
    severity: f64,
) -> Observation {
    evidence.insert(0, origin.as_str().to_string());
    Observation {
        trigger,
        wanting: vec![wanting],
        observation,
        affected_domains: vec![domain.to_string()],
        evidence,
        severity,
        // No detector here measures money. Zero is stated, not defaulted:
        // the curriculum refuses a missing value and ranks a zero honestly.
        economic_value: Decimal::ZERO,
    }
}

/// Belief classes (or agents) whose resolved hit rate is below `floor`.
///
/// `evaluated` is how many resolved predictions the rates were computed on;
/// below `min_evaluated` nothing is raised, because a hit rate over three
/// predictions is noise and a detector that fires on noise trains the
/// curriculum to ignore it. Severity is the shortfall below the floor as a
/// fraction of the floor.
pub fn low_confidence(
    hit_rate_by_class: &BTreeMap<String, f64>,
    evaluated: usize,
    min_evaluated: usize,
    floor: f64,
) -> Result<Vec<Observation>> {
    let floor = fraction("the hit-rate floor", floor)?;
    if evaluated < min_evaluated || floor <= 0.0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (class, rate) in hit_rate_by_class {
        let rate = fraction("a hit rate", *rate)?;
        if rate < floor {
            out.push(observe(
                Origin::LowConfidence,
                GapTrigger::LowConfidence,
                // A class the platform keeps calling wrong is a model gap,
                // not a data gap: the inputs were there and the inference
                // from them failed.
                Wanting::Model,
                format!("belief class `{class}` resolves right {rate:.3} of the time, below the {floor:.3} floor"),
                "beliefs",
                vec![format!("hit_rate_by_class:{class}")],
                (floor - rate) / floor,
            ));
        }
    }
    Ok(out)
}

/// Agents whose resolved hit rate is below `floor`: a capability whose only
/// specialist is not competent at it (§22's sixth gap kind).
pub fn incompetent_specialists(
    hit_rate_by_agent: &BTreeMap<String, f64>,
    evaluated: usize,
    min_evaluated: usize,
    floor: f64,
) -> Result<Vec<Observation>> {
    let floor = fraction("the hit-rate floor", floor)?;
    if evaluated < min_evaluated || floor <= 0.0 {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (agent, rate) in hit_rate_by_agent {
        let rate = fraction("a hit rate", *rate)?;
        if rate < floor {
            out.push(observe(
                Origin::IncompetentSpecialist,
                GapTrigger::UnsupportedRequest,
                Wanting::Specialist,
                format!("agent `{agent}` resolves right {rate:.3} of the time, below the {floor:.3} floor; no competent specialist holds its seat"),
                "agents",
                vec![format!("hit_rate_by_agent:{agent}")],
                (floor - rate) / floor,
            ));
        }
    }
    Ok(out)
}

/// One declined order, scored after the fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decline {
    pub gate: String,
    /// Whether the declined order would have earned.
    pub regret: bool,
}

/// Gates whose declines would have earned at least `min_regrets` times
/// (EXPAND-066, repeated vetoes). Severity is the regretted share of that
/// gate's scored declines.
pub fn repeated_vetoes(declines: &[Decline], min_regrets: usize) -> Result<Vec<Observation>> {
    if min_regrets == 0 {
        return Err(Error::invalid(
            "min_regrets of zero raises a veto gap for every gate that ever declined; state at least one",
        ));
    }
    let mut by_gate: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for d in declines {
        let slot = by_gate.entry(d.gate.as_str()).or_default();
        slot.1 += 1;
        if d.regret {
            slot.0 += 1;
        }
    }
    Ok(by_gate
        .into_iter()
        .filter(|(_, (regrets, _))| *regrets >= min_regrets)
        .map(|(gate, (regrets, scored))| {
            observe(
                Origin::RepeatedVeto,
                GapTrigger::MissedOpportunity,
                // The gate's rule is the model that was wrong.
                Wanting::Model,
                format!("gate `{gate}` declined {regrets} of {scored} scored orders that would have earned"),
                "risk",
                vec![format!("declined_scores:gate:{gate}")],
                regrets as f64 / scored as f64,
            )
        })
        .collect())
}

/// One gap per model reference that drifted past its threshold. Without
/// it the learning round retired a drifted model and the curriculum never
/// heard that the slot it filled now needs a successor fitted. A reference
/// listed twice is one model, so it raises one gap, not two.
pub fn model_drift(drifted: &[String]) -> Vec<Observation> {
    let distinct: BTreeSet<&str> = drifted.iter().map(String::as_str).collect();
    distinct
        .into_iter()
        .map(|reference| {
            observe(
                Origin::ModelDrift,
                GapTrigger::ModelDrift,
                Wanting::Model,
                format!("model `{reference}` drifted past its threshold"),
                "models",
                vec![format!("model:{reference}")],
                1.0,
            )
        })
        .collect()
}

/// A hosted model whose refused or malformed replies exceed `max_rate` of at
/// least `min_replies`, as one surprise wanting a specialist: a model that
/// hallucinates is a question for whoever owns that seat, not a reason to
/// trust its next answer. Below `min_replies` nothing is raised, because a
/// rate over three calls is noise.
pub fn model_error_rate(
    model: &str,
    replies: u64,
    failed: u64,
    min_replies: u64,
    max_rate: f64,
) -> Result<Option<Observation>> {
    let max_rate = fraction("max_rate", max_rate)?;
    if min_replies == 0 || failed > replies {
        return Err(Error::invalid(format!(
            "an error rate needs at least one reply and no more failures than replies, not \
             {failed} of {replies} with a minimum of {min_replies}"
        )));
    }
    if replies < min_replies {
        return Ok(None);
    }
    let rate = failed as f64 / replies as f64;
    if rate <= max_rate {
        return Ok(None);
    }
    Ok(Some(observe(
        Origin::ModelErrorRate,
        GapTrigger::Surprise,
        Wanting::Specialist,
        format!("model `{model}` refused or garbled {failed} of {replies} replies"),
        "models",
        vec![format!("model:{model}")],
        rate,
    )))
}

/// One feasibility or reconciliation refusal, by venue and constraint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub venue: String,
    pub constraint: String,
}

/// (venue, constraint) pairs refused at least `min_refusals` times
/// (EXPAND-066, reconciliation and execution failures). Severity is that
/// pair's share of all refusals read.
pub fn refusal_clusters(refusals: &[Refusal], min_refusals: usize) -> Result<Vec<Observation>> {
    if min_refusals == 0 {
        return Err(Error::invalid(
            "min_refusals of zero raises an execution gap for nothing at all; state at least one",
        ));
    }
    let mut counts: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    for r in refusals {
        *counts
            .entry((r.venue.as_str(), r.constraint.as_str()))
            .or_default() += 1;
    }
    let total = refusals.len();
    Ok(counts
        .into_iter()
        .filter(|(_, n)| *n >= min_refusals)
        .map(|((venue, constraint), n)| {
            observe(
                Origin::RefusalCluster,
                GapTrigger::UnexplainedPnl,
                Wanting::Execution,
                format!("venue `{venue}` refused {n} order(s) on `{constraint}`"),
                "execution",
                vec![format!("feasibility_refusals:{venue}:{constraint}")],
                n as f64 / total as f64,
            )
        })
        .collect())
}

/// A prediction residual: what a twin said and what happened.
#[derive(Debug, Clone, PartialEq)]
pub struct Residual {
    pub subject: String,
    /// Residual in basis points; sign is the direction of the miss.
    pub error_bps: f64,
}

/// Subjects whose mean absolute residual exceeds `threshold_bps` (§22's
/// "unexplained residuals"). Severity saturates at one once the mean miss is
/// ten times the threshold — stated as a ratio, never clamped from outside.
pub fn unexplained_residuals(
    residuals: &[Residual],
    threshold_bps: f64,
) -> Result<Vec<Observation>> {
    if !threshold_bps.is_finite() || threshold_bps <= 0.0 {
        return Err(Error::invalid(format!(
            "the residual threshold must be a positive number of basis points, not {threshold_bps}"
        )));
    }
    let mut by_subject: BTreeMap<&str, (f64, usize)> = BTreeMap::new();
    for r in residuals {
        if !r.error_bps.is_finite() {
            return Err(Error::numeric(format!(
                "a non-finite residual for `{}` reached the detector; drop it at the source",
                r.subject
            )));
        }
        let slot = by_subject.entry(r.subject.as_str()).or_default();
        slot.0 += r.error_bps.abs();
        slot.1 += 1;
    }
    Ok(by_subject
        .into_iter()
        .filter_map(|(subject, (sum, n))| {
            let mean = sum / n as f64;
            (mean > threshold_bps).then(|| {
                observe(
                    Origin::UnexplainedResidual,
                    GapTrigger::UnexplainedPnl,
                    // The twin that predicted the fill is the model missing.
                    Wanting::Model,
                    // The measurement rides in the evidence and the text
                    // stays fixed: the text is the research question, and a
                    // question that changed with every sweep's mean was a
                    // new item each sweep while the item started on the
                    // last one went quiet — a campaign could never reach
                    // its paper stage on a question that kept renaming
                    // itself.
                    format!(
                        "fills at `{subject}` miss the twin past {threshold_bps:.1} bps on average"
                    ),
                    "execution",
                    vec![
                        format!("fill_scores:{subject}"),
                        format!("mean_abs_bps:{mean:.1}"),
                        format!("fills:{n}"),
                    ],
                    (mean / (10.0 * threshold_bps)).min(1.0),
                )
            })
        })
        .collect())
}

/// One directional reading about a subject, from one source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub subject: String,
    pub source: String,
    /// `true` for a reading that says up, `false` for down.
    pub up: bool,
}

/// Subjects about which two sources disagree on direction in one sweep
/// (§22's "contradictory evidence"). What is missing is the causal link that
/// would say which of the two is right. Severity is the minority's share,
/// so a 1-of-10 dissent is a weaker contradiction than a 5-of-10 split.
pub fn contradictions(readings: &[Reading]) -> Vec<Observation> {
    let mut by_subject: BTreeMap<&str, (BTreeSet<&str>, BTreeSet<&str>)> = BTreeMap::new();
    for r in readings {
        let slot = by_subject.entry(r.subject.as_str()).or_default();
        if r.up {
            slot.0.insert(r.source.as_str());
        } else {
            slot.1.insert(r.source.as_str());
        }
    }
    by_subject
        .into_iter()
        .filter(|(_, (up, down))| !up.is_empty() && !down.is_empty())
        .map(|(subject, (up, down))| {
            let minority = up.len().min(down.len()) as f64;
            let total = (up.len() + down.len()) as f64;
            let mut evidence: Vec<String> = up.iter().map(|s| format!("up:{s}")).collect();
            evidence.extend(down.iter().map(|s| format!("down:{s}")));
            observe(
                Origin::Contradiction,
                GapTrigger::Contradiction,
                Wanting::CausalLink,
                format!(
                    "`{subject}` read up by {} and down by {} in the same sweep",
                    up.iter().copied().collect::<Vec<_>>().join(", "),
                    down.iter().copied().collect::<Vec<_>>().join(", ")
                ),
                "market",
                evidence,
                // A two-way split of equal size is the strongest
                // contradiction there is and scores one.
                2.0 * minority / total,
            )
        })
        .collect()
}

/// Asset classes whose instruments have weak history coverage (EXPAND-065,
/// and §22's "unsupported asset classes"). `universe` maps an asset class to
/// its instruments; `covered` is the instruments with history. A class whose
/// covered share is below `floor` raises one observation; a class with none
/// covered says so in its text. Severity is the uncovered share.
pub fn coverage(
    universe: &BTreeMap<String, BTreeSet<String>>,
    covered: &BTreeSet<String>,
    floor: f64,
) -> Result<Vec<Observation>> {
    let floor = fraction("the coverage floor", floor)?;
    let mut out = Vec::new();
    for (class, instruments) in universe {
        if instruments.is_empty() {
            continue;
        }
        let have = instruments.iter().filter(|i| covered.contains(*i)).count();
        let share = have as f64 / instruments.len() as f64;
        if share < floor {
            let text = if have == 0 {
                format!(
                    "asset class `{class}` is unsupported: none of its {} instrument(s) has history",
                    instruments.len()
                )
            } else {
                format!(
                    "asset class `{class}` has history for {have} of {} instrument(s), below the {floor:.2} floor",
                    instruments.len()
                )
            };
            out.push(observe(
                Origin::Coverage,
                GapTrigger::UnsupportedRequest,
                Wanting::Source,
                text,
                "coverage",
                vec![format!("universe:asset_class:{class}")],
                1.0 - share,
            ));
        }
    }
    Ok(out)
}

/// One observation per distinct lack: the shape `gap::classify` asks for when
/// it refuses mixed evidence ("raise one signal for each"). An observation
/// already naming one lack comes back unchanged; one naming nothing comes
/// back unchanged too, so `classify` refuses it by name rather than this
/// function hiding it.
pub fn split_by_wanting(observation: Observation) -> Vec<Observation> {
    let distinct: BTreeSet<Wanting> = observation.wanting.iter().copied().collect();
    if distinct.len() <= 1 {
        return vec![observation];
    }
    distinct
        .into_iter()
        .map(|w| Observation {
            wanting: vec![w],
            ..observation.clone()
        })
        .collect()
}

/// Unknown-unknown surfacing (EXPAND-064): when at least `min_subjects`
/// distinct subjects show the same trigger and the same lack in one sweep,
/// raise one investigation for the cluster, with no human question behind
/// it. An [`Observation`] carries no subject field, so the caller pairs each
/// with the subject it is about. Members naming more than one lack are
/// skipped: split them with [`split_by_wanting`] first.
///
/// Severity is the mean of the members'; evidence lists every member.
pub fn anomaly_clusters(
    members: &[(String, Observation)],
    min_subjects: usize,
) -> Result<Vec<Observation>> {
    if min_subjects < 2 {
        return Err(Error::invalid(
            "a cluster needs at least two subjects; one subject is an anomaly, not a pattern",
        ));
    }
    let mut groups: BTreeMap<(GapTrigger, Wanting), BTreeMap<&str, f64>> = BTreeMap::new();
    for (subject, obs) in members {
        let [wanting] = obs.wanting.as_slice() else {
            continue;
        };
        // `Wanting::Unknown` is grouped like any other lack: an unnamed lack
        // shared by many subjects is still a pattern, and the cluster is
        // what names it, as a missing causal driver.
        groups
            .entry((obs.trigger, *wanting))
            .or_default()
            .entry(subject.as_str())
            .and_modify(|s| *s = s.max(obs.severity))
            .or_insert(obs.severity);
    }
    Ok(groups
        .into_iter()
        .filter(|(_, subjects)| subjects.len() >= min_subjects)
        .map(|((trigger, wanting), subjects)| {
            let mean = subjects.values().sum::<f64>() / subjects.len() as f64;
            let names: Vec<&str> = subjects.keys().copied().collect();
            Observation {
                trigger,
                // Many subjects moving on the same symptom at once is a
                // common driver nobody has modelled, whatever each member
                // thought it lacked.
                wanting: vec![Wanting::CausalLink],
                observation: format!(
                    "{} subjects show the same {trigger:?}/{wanting:?} symptom in one sweep: {}",
                    names.len(),
                    names.join(", ")
                ),
                affected_domains: vec!["market".to_string()],
                evidence: std::iter::once(Origin::AnomalyCluster.as_str().to_string())
                    .chain(names.iter().map(|n| format!("member:{n}")))
                    .collect(),
                severity: mean,
                economic_value: Decimal::ZERO,
            }
        })
        .collect())
}

/// Opportunity-driven curriculum (EXPAND-067): a discrepancy on one subject
/// that persists for `sweeps` consecutive sweeps raises exactly one
/// observation, the sweep it reaches the count. A subject that drops out
/// resets; one that stays past the count raises nothing more until it has
/// dropped out and come back, so a standing discrepancy is one research
/// task and not one per cycle.
#[derive(Debug, Clone)]
pub struct PersistenceTracker {
    sweeps: u32,
    runs: BTreeMap<String, u32>,
}

impl PersistenceTracker {
    pub fn new(sweeps: u32) -> Result<Self> {
        if sweeps < 2 {
            return Err(Error::invalid(
                "persistence needs at least two consecutive sweeps; one sweep is a detection, not a persistent discrepancy",
            ));
        }
        Ok(Self {
            sweeps,
            runs: BTreeMap::new(),
        })
    }

    /// Feed the subjects showing a discrepancy this sweep.
    pub fn observe(&mut self, subjects: &BTreeSet<String>) -> Vec<Observation> {
        self.runs.retain(|s, _| subjects.contains(s));
        let mut out = Vec::new();
        for subject in subjects {
            let run = self.runs.entry(subject.clone()).or_insert(0);
            *run = run.saturating_add(1);
            if *run == self.sweeps {
                out.push(observe(
                    Origin::PersistentDiscrepancy,
                    GapTrigger::MissedOpportunity,
                    // Testing whether a standing discrepancy is real is a
                    // specialist's question before it is anybody's trade.
                    Wanting::Specialist,
                    format!(
                        "`{subject}` has shown a discrepancy for {} consecutive sweeps",
                        self.sweeps
                    ),
                    "market",
                    vec![format!("persistent:{subject}")],
                    1.0,
                ));
            }
        }
        out
    }

    /// Subjects currently being tracked; bounded by the subjects seen in the
    /// latest sweep, because a subject absent from a sweep is dropped.
    pub fn tracked(&self) -> usize {
        self.runs.len()
    }
}
