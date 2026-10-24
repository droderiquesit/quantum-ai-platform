//! The time-aligned fusion of world and market intelligence
//! (R12-TICK-2081–2086, blueprint §8.4).
//!
//! The kernel held a world model and a market, and consumed them separately:
//! an event could be in the graph and the platform could not say whether the
//! market had reacted to it, whether a tradable reaction still existed, or
//! whether the market was disagreeing with what the world model believed.
//! [`fuse`] joins the two **as of one instant**, and the join is the whole
//! point, so its time discipline is stated first.
//!
//! # Nothing is read before it was knowable
//!
//! An event enters the fusion only if it had occurred by `as_of` *and* the
//! platform had recorded it by `as_of` ([`KnowledgeGraph::events_at`] with both
//! instants); its exposure walk asks the graph for facts that held and were
//! known at `as_of`; and a market point is read only if it was knowable at or
//! before `as_of`. The reaction to an event is measured from the last mid
//! knowable when the platform **learned** of the event, not when it occurred:
//! a move between the two is reported separately as
//! [`MeasuredReaction::priced_before_known_bps`], because a reaction the
//! platform could not have traded is not a tradable reaction.
//!
//! # What it produces
//!
//! For each knowable event: a **watch graph** of the entities it concerns and
//! the instruments and venues within two hops of them, with a **prior** that
//! each instrument reacts (2082); the **measured reaction** of each watched
//! instrument from the tick traces — move, time to first material move,
//! peak, decay and spread response — and the **belief update** it implies
//! (2083); and a **mismatch** where the market disagreed with the world
//! model: an expected reaction that is absent raises an investigation, or an
//! evidence-quality challenge when the evidence was weak, and a reaction in
//! the opposite direction raises a relative-value hypothesis (2084). The
//! fused state is what the foundry and the brains read through
//! [`FusedState::posteriors`] (2085), and [`FusedState::opportunity_map`]
//! distils it into the bounded map a regional reflex cell carries (2086),
//! shipped only to regions the training entitlements permit
//! ([`OpportunityMap::authorise_for`]).
//!
//! Everything iterates in key order and nothing reads a clock; the same
//! graph, traces and instant fuse to the same state.

use qip_ai::registry::ModelCard;
use qip_core::error::{Error, Result};
use qip_core::hash::sha256_hex;
use qip_core::{Duration, Timestamp};
use qip_world_model::exposure::{MAX_EXPOSURE_HOPS, instruments_exposed_to};
use qip_world_model::graph::{KnowledgeGraph, NodeKind};
use qip_world_model::relationship::RelationshipKind;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One point of an instrument's replayed market, stamped with the instant
/// it became knowable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TracePoint {
    pub at: Timestamp,
    pub mid: f64,
    pub spread_bps: f64,
    /// Best-bid size plus best-ask size at the point, in the instrument's own
    /// units. `None` where the replay recorded no sizes, which is not zero
    /// depth: a book read as empty and a book never read are different.
    #[serde(default)]
    pub depth: Option<f64>,
    /// `(bid size - ask size) / (bid size + ask size)` in `[-1, 1]`, positive
    /// where the bid side is heavier. `None` with `depth`.
    #[serde(default)]
    pub imbalance: Option<f64>,
}

/// An instrument's market as a tick replay rebuilt it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketTrace {
    /// The instrument's object id — matched against a graph instrument
    /// node's `object_id` attribute, or its node id where it has none.
    pub instrument: String,
    pub venue: String,
    /// In any order; read in time order.
    pub points: Vec<TracePoint>,
}

/// How the fusion judges a reaction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FusionConfig {
    /// Hops from an event's entities to an instrument, at most
    /// [`MAX_EXPOSURE_HOPS`].
    pub max_hops: usize,
    /// A move at least this large, in basis points, is a reaction.
    pub reaction_threshold_bps: f64,
    /// How long after the event became knowable a reaction is waited for
    /// before its absence is a finding.
    pub reaction_window: Duration,
    /// Prior at or above which an absent or contrary reaction is a mismatch.
    pub expect_reaction_above: f64,
    /// Evidence quality below which an absent reaction challenges the
    /// evidence rather than opening an investigation.
    pub weak_evidence_below: f64,
    /// The most events fused per call; more is refused, not truncated.
    pub max_events: usize,
}

impl FusionConfig {
    fn validate(&self) -> Result<()> {
        if self.max_hops == 0 || self.max_hops > MAX_EXPOSURE_HOPS {
            return Err(Error::invalid(format!(
                "fusion walks 1..={MAX_EXPOSURE_HOPS} hops from an event's entities; {} is not \
                 an exposure",
                self.max_hops
            )));
        }
        if !self.reaction_threshold_bps.is_finite() || self.reaction_threshold_bps <= 0.0 {
            return Err(Error::invalid(
                "a reaction threshold is a positive number of basis points",
            ));
        }
        if self.reaction_window <= Duration::ZERO {
            return Err(Error::invalid("a reaction window must be positive"));
        }
        for (name, value) in [
            ("expect_reaction_above", self.expect_reaction_above),
            ("weak_evidence_below", self.weak_evidence_below),
        ] {
            if !(0.0..=1.0).contains(&value) {
                return Err(Error::invalid(format!(
                    "fusion {name} is a probability in [0, 1], not {value}"
                )));
            }
        }
        if self.max_events == 0 {
            return Err(Error::invalid(
                "a fusion bound of zero events fuses nothing",
            ));
        }
        Ok(())
    }
}

/// One instrument an event reaches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WatchedInstrument {
    /// The market object id the instrument trades under.
    pub instrument: String,
    pub hops: usize,
    /// Entity first, instrument last.
    pub path: Vec<String>,
    /// Product of the confidences walked.
    pub exposure_confidence: f64,
    /// The venues its trace was recorded on.
    pub venues: BTreeSet<String>,
}

/// What an event is expected to touch (2082).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WatchGraph {
    pub entities: BTreeSet<String>,
    pub instruments: BTreeMap<String, WatchedInstrument>,
}

/// The world model's prior that an instrument reacts, before the market is read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HypothesisPrior {
    pub instrument: String,
    /// `+1` up, `-1` down, `0` where the event states no direction.
    pub expected_direction: f64,
    pub probability: f64,
}

/// The market's reaction to one event in one instrument (2083).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeasuredReaction {
    pub instrument: String,
    /// The mid knowable when the platform learned of the event.
    pub baseline_mid: f64,
    pub latest_mid: f64,
    /// Latest against baseline, in basis points.
    pub move_bps: f64,
    /// Largest absolute move after the baseline, signed.
    pub peak_move_bps: f64,
    /// Share of the peak still present, where there was a peak.
    pub decay: Option<f64>,
    /// From learning of the event to the first material move.
    pub first_move_after: Option<Duration>,
    /// Spread at the latest point less spread at the baseline.
    pub spread_change_bps: f64,
    /// Move between the event occurring and the platform learning of it —
    /// already priced, and not tradable by the platform.
    pub priced_before_known_bps: Option<f64>,
    pub reacted: bool,
    /// Depth at the latest point over depth at the baseline: below one the
    /// book thinned under the news. `None` where either point recorded none.
    #[serde(default)]
    pub depth_ratio: Option<f64>,
    /// Book imbalance at the latest point, signed toward the bid.
    #[serde(default)]
    pub flow_imbalance: Option<f64>,
    /// Standard deviation of the tick-to-tick mid returns after the baseline,
    /// in basis points: the regime the reaction happened in. `None` below
    /// three points, where a deviation says nothing.
    #[serde(default)]
    pub realised_vol_bps: Option<f64>,
    /// Gap between the highest and lowest venue mid for this instrument at
    /// `as_of`, over their mean, in basis points; `None` on one venue.
    #[serde(default)]
    pub venue_dislocation_bps: Option<f64>,
    /// Whether the move at the latest point is larger than the spread that
    /// crossing it costs: a reaction smaller than its own cost is not
    /// something the platform could have acted on.
    #[serde(default)]
    pub clears_spread: bool,
}

/// A belief moved by a measured reaction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BeliefUpdate {
    pub instrument: String,
    pub prior: f64,
    pub posterior: f64,
    /// Whether the reaction's sign agreed with the expected direction, where
    /// one was expected and a reaction happened.
    pub direction_agreed: Option<bool>,
}

/// Where the market disagreed with the world model (2084).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "response")]
pub enum Mismatch {
    /// An expected reaction is absent and the evidence was sound.
    Investigation { instrument: String, reason: String },
    /// The market moved against the expected direction.
    RelativeValueHypothesis {
        instrument: String,
        expected_direction: f64,
        measured_bps: f64,
    },
    /// An expected reaction is absent and the evidence was weak.
    EvidenceQualityChallenge {
        evidence_quality: f64,
        reason: String,
    },
}

/// One event, fused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FusedEvent {
    pub event: String,
    pub occurred_at: Timestamp,
    /// When the platform recorded it.
    pub knowable_at: Timestamp,
    pub evidence_quality: f64,
    pub watch: WatchGraph,
    pub priors: Vec<HypothesisPrior>,
    pub reactions: Vec<MeasuredReaction>,
    pub beliefs: Vec<BeliefUpdate>,
    pub mismatches: Vec<Mismatch>,
}

/// The fused state at one instant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FusedState {
    pub as_of: Timestamp,
    pub events: Vec<FusedEvent>,
    /// Canonical digest of the events, for a replay to compare.
    pub digest: String,
}

/// One entry of the distilled map a reflex cell carries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpportunityEntry {
    pub instrument: String,
    /// Posterior that a reaction is live, signed by its expected direction.
    pub score: f64,
    /// The event it came from.
    pub event: String,
    pub expires_at: Timestamp,
}

/// The distilled fused state (2086): bounded, ranked, expiring, and naming
/// the model card whose entitlements decide where it may be shipped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpportunityMap {
    pub as_of: Timestamp,
    pub entries: Vec<OpportunityEntry>,
    pub source_digest: String,
}

impl OpportunityMap {
    /// Refuse to ship the map to `region` unless `card`'s training
    /// entitlements permit the region (R12-TICK-2091). The map is derived
    /// from licensed tick data, so it is the licence's to place, not the
    /// sender's.
    pub fn authorise_for(&self, card: &ModelCard, region: &str) -> Result<()> {
        card.authorise_location(region)
    }
}

fn instrument_key(graph: &KnowledgeGraph, node: &str) -> String {
    graph
        .node(node)
        .and_then(|n| n.attributes.get("object_id").cloned())
        .unwrap_or_else(|| node.to_string())
}

fn direction_of(text: Option<&String>) -> f64 {
    match text.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("up" | "+1" | "1" | "positive") => 1.0,
        Some("down" | "-1" | "negative") => -1.0,
        _ => 0.0,
    }
}

fn last_at_or_before(points: &[&TracePoint], at: Timestamp) -> Option<usize> {
    let end = points.partition_point(|p| p.at <= at);
    end.checked_sub(1)
}

/// Standard deviation of consecutive mid returns, in basis points.
fn realised_vol_bps(points: &[&TracePoint]) -> Option<f64> {
    let returns: Vec<f64> = points
        .windows(2)
        .map(|w| (w[1].mid / w[0].mid - 1.0) * 10_000.0)
        .collect();
    if returns.len() < 2 {
        return None;
    }
    let n = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / n;
    let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
    Some(variance.sqrt())
}

/// The spread of venue mids for `instrument` at `as_of`: each venue's last
/// knowable mid, highest less lowest over the mean.
fn venue_dislocation_bps(
    traces: &[MarketTrace],
    instrument: &str,
    as_of: Timestamp,
) -> Option<f64> {
    let mut by_venue: BTreeMap<&str, (Timestamp, f64)> = BTreeMap::new();
    for trace in traces.iter().filter(|t| t.instrument == instrument) {
        for p in trace
            .points
            .iter()
            .filter(|p| p.at <= as_of && p.mid.is_finite() && p.mid > 0.0)
        {
            let held = by_venue
                .entry(trace.venue.as_str())
                .or_insert((p.at, p.mid));
            if p.at >= held.0 {
                *held = (p.at, p.mid);
            }
        }
    }
    if by_venue.len() < 2 {
        return None;
    }
    let mids: Vec<f64> = by_venue.values().map(|(_, mid)| *mid).collect();
    let high = mids.iter().copied().fold(f64::MIN, f64::max);
    let low = mids.iter().copied().fold(f64::MAX, f64::min);
    let mean = mids.iter().sum::<f64>() / mids.len() as f64;
    Some((high - low) / mean * 10_000.0)
}

/// Fuse the world model and the market traces as of `as_of`. See the module
/// doc. Refuses more knowable events than `config.max_events`.
pub fn fuse(
    graph: &KnowledgeGraph,
    traces: &[MarketTrace],
    as_of: Timestamp,
    config: &FusionConfig,
) -> Result<FusedState> {
    config.validate()?;
    // Market side, knowable points only, in time order.
    let mut market: BTreeMap<&str, (BTreeSet<String>, Vec<&TracePoint>)> = BTreeMap::new();
    for trace in traces {
        let entry = market.entry(trace.instrument.as_str()).or_default();
        entry.0.insert(trace.venue.clone());
        entry.1.extend(
            trace
                .points
                .iter()
                .filter(|p| p.at <= as_of && p.mid.is_finite() && p.mid > 0.0),
        );
    }
    for (_, points) in market.values_mut() {
        points.sort_by_key(|p| p.at);
    }

    let mut events: Vec<_> = graph.events_at(as_of, as_of);
    events.sort_by(|a, b| a.id.cmp(&b.id));
    if events.len() > config.max_events {
        return Err(Error::invalid(format!(
            "{} events were knowable at {as_of}, more than the {} this fusion may hold; narrow \
             the window rather than fuse a prefix",
            events.len(),
            config.max_events
        )));
    }

    let mut fused = Vec::new();
    for event in events {
        let Some(occurred_at) = event.occurred_at else {
            continue;
        };
        let knowable_at = event.recorded_at;
        let concerns: Vec<_> = graph.neighbours(
            &event.id,
            Some(RelationshipKind::ConcernsEntity),
            as_of,
            as_of,
        );
        let entities: BTreeSet<String> =
            concerns.iter().map(|f| f.relationship.to.clone()).collect();
        let fact_confidence = concerns
            .iter()
            .map(|f| f.confidence)
            .fold(1.0_f64, f64::min);
        let evidence_quality = event
            .attributes
            .get("evidence_quality")
            .and_then(|q| q.parse::<f64>().ok())
            .filter(|q| (0.0..=1.0).contains(q))
            .unwrap_or(fact_confidence);
        let direction = direction_of(event.attributes.get("expected_direction"));

        let mut watched: BTreeMap<String, WatchedInstrument> = BTreeMap::new();
        for entity in &entities {
            let exposures = if graph
                .node(entity)
                .is_some_and(|n| n.kind == NodeKind::FinancialObject)
            {
                // An event concerning an instrument directly.
                vec![(entity.clone(), 0usize, vec![entity.clone()], 1.0)]
            } else {
                instruments_exposed_to(graph, entity, config.max_hops, as_of, as_of)?
                    .exposures
                    .into_iter()
                    .map(|e| (e.instrument, e.hops, e.path, e.confidence))
                    .collect()
            };
            for (node, hops, path, confidence) in exposures {
                let key = instrument_key(graph, &node);
                let venues = market
                    .get(key.as_str())
                    .map(|(v, _)| v.clone())
                    .unwrap_or_default();
                watched
                    .entry(key.clone())
                    .and_modify(|w| {
                        if confidence > w.exposure_confidence {
                            w.exposure_confidence = confidence;
                            w.hops = hops;
                            w.path = path.clone();
                        }
                    })
                    .or_insert(WatchedInstrument {
                        instrument: key,
                        hops,
                        path,
                        exposure_confidence: confidence,
                        venues,
                    });
            }
        }

        let mut priors = Vec::new();
        let mut reactions = Vec::new();
        let mut beliefs = Vec::new();
        let mut mismatches = Vec::new();
        let window_elapsed = as_of >= knowable_at.saturating_add(config.reaction_window);
        let mut absent_expected = Vec::new();
        for (instrument, watch) in &watched {
            // Both factors are in [0, 1] by construction: fact confidences are
            // refused outside it at the write seam and the evidence quality
            // attribute is read only inside it.
            let prior = watch.exposure_confidence * evidence_quality;
            priors.push(HypothesisPrior {
                instrument: instrument.clone(),
                expected_direction: direction,
                probability: prior,
            });
            let Some((_, points)) = market.get(instrument.as_str()) else {
                continue;
            };
            let Some(base_index) = last_at_or_before(points, knowable_at) else {
                continue;
            };
            let base = points[base_index];
            let after = &points[base_index..];
            let Some(latest) = after.last() else {
                continue;
            };
            let bps = |mid: f64| (mid / base.mid - 1.0) * 10_000.0;
            let move_bps = bps(latest.mid);
            let peak_move_bps = after
                .iter()
                .map(|p| bps(p.mid))
                .max_by(|a, b| a.abs().total_cmp(&b.abs()))
                .unwrap_or(0.0);
            let first_move_after = after
                .iter()
                .find(|p| bps(p.mid).abs() >= config.reaction_threshold_bps)
                .map(|p| p.at.since(knowable_at));
            let decay = (peak_move_bps.abs() >= config.reaction_threshold_bps)
                .then(|| move_bps / peak_move_bps);
            let priced_before_known_bps = last_at_or_before(points, occurred_at)
                .filter(|i| *i < base_index)
                .map(|i| (base.mid / points[i].mid - 1.0) * 10_000.0);
            let reacted = first_move_after.is_some();
            reactions.push(MeasuredReaction {
                instrument: instrument.clone(),
                baseline_mid: base.mid,
                latest_mid: latest.mid,
                move_bps,
                peak_move_bps,
                decay,
                first_move_after,
                spread_change_bps: latest.spread_bps - base.spread_bps,
                priced_before_known_bps,
                reacted,
                depth_ratio: match (base.depth, latest.depth) {
                    (Some(b), Some(l)) if b > 0.0 && b.is_finite() && l.is_finite() => Some(l / b),
                    _ => None,
                },
                flow_imbalance: latest.imbalance,
                realised_vol_bps: realised_vol_bps(after),
                venue_dislocation_bps: venue_dislocation_bps(traces, instrument, as_of),
                clears_spread: move_bps.abs() > latest.spread_bps,
            });
            // Beta-Bernoulli with two pseudo-observations at the prior: one
            // reaction moves a belief, it does not replace it.
            let (a, b) = (2.0 * prior, 2.0 * (1.0 - prior));
            let direction_agreed =
                (reacted && direction != 0.0).then(|| peak_move_bps.signum() == direction.signum());
            let success = reacted && direction_agreed != Some(false);
            let posterior = if success {
                (a + 1.0) / (a + b + 1.0)
            } else {
                a / (a + b + 1.0)
            };
            beliefs.push(BeliefUpdate {
                instrument: instrument.clone(),
                prior,
                posterior,
                direction_agreed,
            });
            if prior >= config.expect_reaction_above {
                if direction_agreed == Some(false) {
                    mismatches.push(Mismatch::RelativeValueHypothesis {
                        instrument: instrument.clone(),
                        expected_direction: direction,
                        measured_bps: peak_move_bps,
                    });
                } else if !reacted && window_elapsed {
                    absent_expected.push(instrument.clone());
                }
            }
        }
        if !absent_expected.is_empty() {
            let reason = format!(
                "{} expected to react within {}s of {} and did not: {}",
                event.id,
                config.reaction_window.as_secs_f64(),
                knowable_at,
                absent_expected.join(", ")
            );
            if evidence_quality < config.weak_evidence_below {
                mismatches.push(Mismatch::EvidenceQualityChallenge {
                    evidence_quality,
                    reason,
                });
            } else {
                for instrument in absent_expected {
                    mismatches.push(Mismatch::Investigation {
                        instrument,
                        reason: reason.clone(),
                    });
                }
            }
        }
        fused.push(FusedEvent {
            event: event.id.clone(),
            occurred_at,
            knowable_at,
            evidence_quality,
            watch: WatchGraph {
                entities,
                instruments: watched,
            },
            priors,
            reactions,
            beliefs,
            mismatches,
        });
    }
    let digest = sha256_hex(&serde_json::to_vec(&fused)?);
    Ok(FusedState {
        as_of,
        events: fused,
        digest,
    })
}

/// A belief about one instrument that comes from outside the opportunity
/// under review, and may only narrow REASON (ADR 0151 decision 3).
///
/// A fused posterior is a belief about an instrument reacting, not evidence
/// the panel weighed. Letting it raise a hypothesis's confidence would put an
/// unreviewed statistic one approval away from a position size, so the only
/// effect it has is [`OutsidePrior::narrowing_for`]: a multiplier in `[0, 1]`
/// that is exactly 1 unless the posterior leans against the hypothesis.
#[derive(Clone, Debug, PartialEq)]
pub struct OutsidePrior {
    subject: String,
    direction: f64,
    probability: f64,
}

impl OutsidePrior {
    /// `direction` is `+1.0` or `-1.0`, the way the posterior expects the
    /// instrument to move; `probability` is the posterior that it does.
    ///
    /// Refused rather than clamped: a probability outside `[0, 1]` or a
    /// direction of zero is a producer bug, and a silently corrected one would
    /// survive it.
    pub fn new(subject: impl Into<String>, direction: f64, probability: f64) -> Result<Self> {
        let subject = subject.into();
        if subject.is_empty() {
            return Err(Error::invalid("an outside prior needs a subject"));
        }
        if !matches!(direction, 1.0 | -1.0) {
            return Err(Error::invalid(
                "an outside prior's direction is +1 or -1; a posterior with no expected direction has nothing to narrow against",
            ));
        }
        if !(0.0..=1.0).contains(&probability) {
            return Err(Error::invalid(
                "an outside prior's probability is a probability: between zero and one",
            ));
        }
        Ok(Self {
            subject,
            direction,
            probability,
        })
    }

    /// The instrument the posterior is about.
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// The multiplier on a hypothesis whose own direction is `sign`.
    ///
    /// `m = 1 - max(0, 2p - 1)` with `p` the posterior's probability *against*
    /// the hypothesis. It is derived from the posterior alone and holds no
    /// constant. It is 1 whenever `p <= 0.5`, falls to 0 as the posterior
    /// becomes certain against, and never exceeds 1, so it cannot raise.
    pub fn narrowing_for(&self, sign: f64) -> f64 {
        let against = if sign * self.direction > 0.0 {
            1.0 - self.probability
        } else {
            self.probability
        };
        1.0 - (2.0f64.mul_add(against, -1.0)).max(0.0)
    }
}

impl FusedState {
    /// The posterior per instrument across every event, the strongest kept:
    /// the form the foundry and the asset, capital and risk brains read
    /// (2085). Keyed so iteration order is the instrument order.
    pub fn posteriors(&self) -> BTreeMap<String, f64> {
        let mut out: BTreeMap<String, f64> = BTreeMap::new();
        for event in &self.events {
            for belief in &event.beliefs {
                let entry = out.entry(belief.instrument.clone()).or_insert(0.0);
                if belief.posterior > *entry {
                    *entry = belief.posterior;
                }
            }
        }
        out
    }

    /// The posteriors as [`OutsidePrior`]s: per instrument, the strongest
    /// belief among events whose prior stated a direction. An event that
    /// states none (`expected_direction` of zero) gives REASON nothing to be
    /// against and is left out.
    pub fn outside_priors(&self) -> Vec<OutsidePrior> {
        let mut strongest: BTreeMap<&str, (f64, f64)> = BTreeMap::new();
        for event in &self.events {
            for belief in &event.beliefs {
                let Some(prior) = event
                    .priors
                    .iter()
                    .find(|prior| prior.instrument == belief.instrument)
                else {
                    continue;
                };
                // Zero states no direction: nothing to be against.
                if prior.expected_direction == 0.0 {
                    continue;
                }
                let direction = prior.expected_direction.signum();
                let entry = strongest
                    .entry(belief.instrument.as_str())
                    .or_insert((direction, 0.0));
                if belief.posterior > entry.1 {
                    *entry = (direction, belief.posterior);
                }
            }
        }
        strongest
            .into_iter()
            .filter_map(|(instrument, (direction, probability))| {
                OutsidePrior::new(instrument, direction, probability).ok()
            })
            .collect()
    }

    /// Every mismatch, with its event, in event order.
    pub fn mismatches(&self) -> Vec<(&str, &Mismatch)> {
        self.events
            .iter()
            .flat_map(|e| e.mismatches.iter().map(move |m| (e.event.as_str(), m)))
            .collect()
    }

    /// Distil into at most `capacity` entries, ranked by the strength of a
    /// still-live reaction, expiring `ttl` after `as_of` (2086). An entry
    /// whose reaction has fully decayed is not live and is left out.
    pub fn opportunity_map(&self, capacity: usize, ttl: Duration) -> Result<OpportunityMap> {
        if capacity == 0 || ttl <= Duration::ZERO {
            return Err(Error::invalid(
                "an opportunity map needs a positive capacity and time to live",
            ));
        }
        let mut entries = Vec::new();
        for event in &self.events {
            let direction = event.priors.first().map_or(0.0, |p| p.expected_direction);
            for belief in &event.beliefs {
                let live = event
                    .reactions
                    .iter()
                    .find(|r| r.instrument == belief.instrument)
                    .is_none_or(|r| r.decay.is_none_or(|d| d.abs() > 0.25));
                if !live {
                    continue;
                }
                let sign = if direction == 0.0 { 1.0 } else { direction };
                entries.push(OpportunityEntry {
                    instrument: belief.instrument.clone(),
                    score: belief.posterior * sign,
                    event: event.event.clone(),
                    expires_at: self.as_of.saturating_add(ttl),
                });
            }
        }
        entries.sort_by(|a, b| {
            b.score
                .abs()
                .total_cmp(&a.score.abs())
                .then_with(|| a.instrument.cmp(&b.instrument))
                .then_with(|| a.event.cmp(&b.event))
        });
        entries.truncate(capacity);
        Ok(OpportunityMap {
            as_of: self.as_of,
            entries,
            source_digest: self.digest.clone(),
        })
    }
}
