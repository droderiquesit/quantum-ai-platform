//! The opportunity engine.
//!
//! Runs the detectors, groups what they found, ranks it, and emits the survivors
//! as opportunities. The ranking is where restraint lives: novelty decays with
//! repetition, so the fifth alert about the same instrument this week ranks far
//! below the first, and confidence gates what reaches an agent at all.

use qip_core::{Context, Duration, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::detector::{Anomaly, AnomalyKind, DetectionContext, DetectorRegistry, horizon_for};
use crate::opportunity::{Investigation, Opportunity, OpportunityRank};

/// How the engine behaves.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EngineConfig {
    /// Minimum rank score for an opportunity to be emitted at all.
    pub minimum_score: f64,
    /// Minimum confidence for an opportunity to reach an agent.
    pub minimum_confidence: f64,
    /// Most opportunities emitted per cycle, however many anomalies fire.
    ///
    /// A hard cap, not a soft preference. A research organisation with an
    /// unbounded queue works through none of it carefully.
    pub max_per_cycle: usize,
    /// How long a subject stays "recently seen" for novelty purposes.
    pub novelty_window: Duration,
    /// How long an opportunity remains actionable.
    pub time_to_live: Duration,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            minimum_score: 0.25,
            minimum_confidence: 0.35,
            max_per_cycle: 12,
            novelty_window: Duration::from_days(7),
            time_to_live: Duration::from_days(5),
        }
    }
}

/// Turns anomalies into ranked opportunities.
#[derive(Debug)]
pub struct OpportunityEngine {
    detectors: DetectorRegistry,
    config: EngineConfig,
    /// When each subject was last reported, for novelty decay.
    recently_seen: BTreeMap<String, Vec<Timestamp>>,
    /// Total emitted, for reporting.
    emitted: u64,
    /// Total suppressed by the cap or the thresholds.
    suppressed: u64,
}

impl Default for OpportunityEngine {
    fn default() -> Self {
        Self::new(DetectorRegistry::standard(), EngineConfig::default())
    }
}

impl OpportunityEngine {
    pub fn new(detectors: DetectorRegistry, config: EngineConfig) -> Self {
        Self {
            detectors,
            config,
            recently_seen: BTreeMap::new(),
            emitted: 0,
            suppressed: 0,
        }
    }

    pub fn detectors(&self) -> &DetectorRegistry {
        &self.detectors
    }

    pub fn emitted_count(&self) -> u64 {
        self.emitted
    }

    pub fn suppressed_count(&self) -> u64 {
        self.suppressed
    }

    /// Expected false positives per thousand observations across all detectors.
    pub fn expected_noise_rate(&self) -> f64 {
        self.detectors.expected_noise_rate()
    }

    /// Run one detection cycle.
    pub fn scan(&mut self, detection: &DetectionContext, context: &Context) -> Vec<Opportunity> {
        let anomalies = self.detectors.detect_all(detection);
        if anomalies.is_empty() {
            return Vec::new();
        }

        // Group by subject: several detectors firing on one instrument is one
        // opportunity with corroborating evidence, not several to work through.
        let mut grouped: BTreeMap<String, Vec<Anomaly>> = BTreeMap::new();
        for anomaly in anomalies {
            grouped
                .entry(primary_subject(&anomaly))
                .or_default()
                .push(anomaly);
        }

        let mut candidates: Vec<Opportunity> = grouped
            .into_iter()
            .filter_map(|(subject, group)| self.build(&subject, group, detection.as_of, context))
            .collect();

        candidates.sort_by(|a, b| {
            b.rank
                .value_per_cost()
                .partial_cmp(&a.rank.value_per_cost())
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.opportunity_id.cmp(&b.opportunity_id))
        });

        let total = candidates.len();
        candidates.truncate(self.config.max_per_cycle);
        self.suppressed += (total - candidates.len()) as u64;
        self.emitted += candidates.len() as u64;

        for opportunity in &candidates {
            for subject in &opportunity.affected_entities {
                self.recently_seen
                    .entry(subject.clone())
                    .or_default()
                    .push(detection.as_of);
            }
        }
        self.prune_history(detection.as_of);
        candidates
    }

    fn build(
        &self,
        subject: &str,
        anomalies: Vec<Anomaly>,
        at: Timestamp,
        context: &Context,
    ) -> Option<Opportunity> {
        if anomalies.is_empty() {
            return None;
        }

        // Corroboration: several independent detectors agreeing is stronger
        // evidence than one firing loudly, so confidence combines rather than
        // taking a maximum.
        let confidence = combine_confidence(&anomalies);
        let importance = anomalies
            .iter()
            .map(Anomaly::importance)
            .fold(0.0f64, f64::max);
        let novelty = self.novelty_of(subject, at);
        let score = importance * confidence * novelty;

        if score < self.config.minimum_score || confidence < self.config.minimum_confidence {
            return None;
        }

        let mut investigations: Vec<Investigation> = anomalies
            .iter()
            .flat_map(|a| a.kind.implied_investigation())
            .collect();
        investigations.sort_by_key(|i| i.as_str());
        investigations.dedup_by_key(|i| i.as_str());
        let cost: f64 = investigations
            .iter()
            .map(Investigation::estimated_cost)
            .sum();

        let horizon = anomalies
            .iter()
            .map(|a| horizon_for(a.kind))
            .max()
            .unwrap_or(Duration::from_days(5));

        let strongest = anomalies
            .iter()
            .max_by(|a, b| {
                a.z_score
                    .abs()
                    .partial_cmp(&b.z_score.abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })?
            .clone();

        let detectors: Vec<String> = {
            let mut names: Vec<String> = anomalies.iter().map(|a| a.detector.clone()).collect();
            names.sort();
            names.dedup();
            names
        };

        Some(Opportunity {
            opportunity_id: context.ids().generate(at),
            detected_at: at,
            headline: strongest.description.clone(),
            affected_objects: affected_objects(&anomalies),
            affected_entities: vec![subject.to_string()],
            evidence: anomalies
                .iter()
                .map(|a| format!("{}:{}", a.detector, a.subject))
                .collect(),
            historical_context: self.historical_context(subject, at, &anomalies),
            rank: OpportunityRank {
                score,
                importance,
                novelty,
                confidence,
                cost,
            },
            horizon,
            required_investigation: investigations,
            detectors,
            anomalies,
            expires_at: at.saturating_add(self.config.time_to_live),
        })
    }

    /// How novel this subject is, given how often it has fired recently.
    ///
    /// Decays geometrically. The first alert about an instrument is fully
    /// novel; the fifth this week is largely a repeat, and ranking it as though
    /// it were new is how a queue fills with the same story.
    fn novelty_of(&self, subject: &str, at: Timestamp) -> f64 {
        let cutoff = at.saturating_sub(self.config.novelty_window);
        let recent = self
            .recently_seen
            .get(subject)
            .map(|times| times.iter().filter(|t| **t >= cutoff).count())
            .unwrap_or(0);
        0.6f64.powi(recent as i32).max(0.05)
    }

    fn historical_context(&self, subject: &str, at: Timestamp, anomalies: &[Anomaly]) -> String {
        let cutoff = at.saturating_sub(self.config.novelty_window);
        let recent = self
            .recently_seen
            .get(subject)
            .map(|times| times.iter().filter(|t| **t >= cutoff).count())
            .unwrap_or(0);
        let kinds: Vec<&str> = {
            let mut kinds: Vec<&str> = anomalies.iter().map(|a| a.kind.as_str()).collect();
            kinds.sort_unstable();
            kinds.dedup();
            kinds
        };
        if recent == 0 {
            format!(
                "First alert on {subject} in the past week; {} detector signal(s): {}",
                anomalies.len(),
                kinds.join(", ")
            )
        } else {
            format!(
                "{recent} prior alert(s) on {subject} within the novelty window, so this is \
                 largely a continuation; signals: {}",
                kinds.join(", ")
            )
        }
    }

    fn prune_history(&mut self, now: Timestamp) {
        let cutoff = now.saturating_sub(self.config.novelty_window * 4);
        for times in self.recently_seen.values_mut() {
            times.retain(|t| *t >= cutoff);
        }
        self.recently_seen.retain(|_, times| !times.is_empty());
    }

    /// Reset the novelty history, for a fresh backtest run.
    pub fn reset(&mut self) {
        self.recently_seen.clear();
        self.emitted = 0;
        self.suppressed = 0;
    }
}

/// The subject an anomaly is really about.
///
/// A correlation breakdown names a pair; the opportunity is attributed to the
/// first leg so it groups with that instrument's other signals.
fn primary_subject(anomaly: &Anomaly) -> String {
    match anomaly.subject.split_once('|') {
        Some((first, _)) => first.to_string(),
        None => anomaly.subject.clone(),
    }
}

fn affected_objects(anomalies: &[Anomaly]) -> Vec<qip_core::ObjectId> {
    let mut subjects: Vec<String> = anomalies
        .iter()
        .flat_map(|a| a.subject.split('|').map(str::to_string).collect::<Vec<_>>())
        .collect();
    subjects.sort();
    subjects.dedup();
    subjects
        .into_iter()
        .map(qip_core::ObjectId::from_string)
        .collect()
}

/// Combine several detectors' confidences.
///
/// Treated as independent evidence: the probability that *none* of them is
/// right falls with each one, so two detectors at 0.6 give more than 0.6 but
/// less than 1.2. Taking the maximum would ignore corroboration; summing would
/// let three weak signals outrank one strong one.
fn combine_confidence(anomalies: &[Anomaly]) -> f64 {
    let complement: f64 = anomalies
        .iter()
        .map(|a| 1.0 - a.confidence().clamp(0.0, 0.99))
        .product();
    (1.0 - complement).clamp(0.0, 1.0)
}

/// Anomaly kinds the engine can produce, for documentation and the UI.
pub fn supported_anomaly_kinds() -> Vec<AnomalyKind> {
    vec![
        AnomalyKind::PriceMove,
        AnomalyKind::VolatilityShift,
        AnomalyKind::VolumeSpike,
        AnomalyKind::StructuralBreak,
        AnomalyKind::RegimeChange,
        AnomalyKind::CorrelationBreakdown,
        AnomalyKind::LiquidityDeterioration,
        AnomalyKind::FundamentalSurprise,
        AnomalyKind::MacroSurprise,
        AnomalyKind::SentimentShift,
        AnomalyKind::AlternativeDataDivergence,
    ]
}
