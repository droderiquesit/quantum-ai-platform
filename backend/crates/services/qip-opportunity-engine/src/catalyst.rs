//! Event impact and catalyst detection.
//!
//! The statistical detectors answer "is this move unusual?". This module
//! answers the question next to it: "when a price moves, was there a knowable
//! event that explains it — and when an event lands, what does it historically
//! do to the instruments it touches?". Three findings come out of it:
//!
//! * an **explained move** — a large move with a plausibly-linked prior event,
//!   the linkage recorded on the anomaly as a [`CatalystLink`] carrying the
//!   event, its known-time and the lag;
//! * a **catalyst landing** — an event of a class whose historical impact on
//!   the instrument is estimable, stated as a magnitude distribution, hit rate
//!   and typical lag rather than as a headline;
//! * an **unexplained move** — a large move with *no* knowable catalyst. This
//!   is kept deliberately distinct from the explained case, because it is the
//!   anomaly most worth escalating: a move the market can point at a filing is
//!   ordinary repricing, while a move nothing public explains is either
//!   information leaking or a data error, and both demand investigation.
//!
//! Two disciplines hold structurally rather than by convention:
//!
//! * **No look-ahead through an event.** An event may only explain a move that
//!   begins after the event's *known-time* — not its valid-time: a filing
//!   about last quarter is knowable only when filed. [`KnownEvents`], the only
//!   event container a [`crate::detector::DetectionContext`] carries, admits
//!   events solely through a constructor that filters by a stated known-by
//!   time, and [`MarketEvent::new`] clamps a known-time earlier than the
//!   occurrence forward — the same discipline as the world model's
//!   `absorb_bar`, where availability is clamped so nothing is readable before
//!   it existed. The detector then additionally requires the known-time to be
//!   strictly before the move window when linking.
//! * **No fabricated statistics.** [`ImpactHistory`] refuses to record an
//!   event→move pair whose ordering did not hold, and refuses to state an
//!   impact for a class with fewer observations than its stated minimum.
//!   Insufficient history is an answer, not a zero: reporting "0% typical
//!   move" for a class seen twice would be a lie the ranking believes.
//!
//! The detector also refuses to call a move *unexplained* when the context
//! carries no event visibility at all. "No events were supplied" and "no
//! catalyst existed" are different claims, and only the second is evidence.

use qip_core::{Duration, Timestamp};
use qip_financial::intelligence::{FundamentalUpdate, MacroObservation, NewsItem};
use qip_numerics::stats;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::detector::{Anomaly, AnomalyKind, DetectionContext, Detector};

/// A discrete, knowable happening attached to an instrument or entity.
///
/// Bitemporal: `occurred_at` is when the event was true in the world, and
/// `known_at` is when this platform could first have acted on it. The two are
/// private because the whole module rests on their ordering: the constructor
/// clamps a known-time earlier than the occurrence forward, and nothing may
/// widen the gap afterwards. A deserialized event is trusted to have been
/// serialized from a clamped one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarketEvent {
    /// Stable identity, cited as evidence by anomalies that link to it.
    pub event_id: String,
    /// Instrument or entity the event lands on. Must match a priced subject in
    /// the detection context for the detector to assess it.
    pub subject: String,
    /// Classification the impact history is keyed by, e.g. `earnings`,
    /// `guidance`, `monetary_policy`, `m&a`.
    pub class: String,
    /// Directional hint in `[-1, 1]`, e.g. sentiment or surprise sign. Zero
    /// means no direction is claimed.
    pub direction: f64,
    /// Source reliability weight in `[0, 1]`, used to pick among competing
    /// explanations.
    pub weight: f64,
    /// Human-readable statement of what happened.
    pub description: String,
    occurred_at: Timestamp,
    known_at: Timestamp,
}

impl MarketEvent {
    /// Build an event, clamping the known-time forward to the occurrence.
    ///
    /// A thing cannot have been knowable before it happened; the combination
    /// always means a mis-stamped record rather than a prescient feed. A
    /// scheduled future happening that *is* knowable in advance should be
    /// modelled as its announcement — the announcement is the knowable event.
    pub fn new(
        event_id: impl Into<String>,
        subject: impl Into<String>,
        class: impl Into<String>,
        occurred_at: Timestamp,
        known_at: Timestamp,
    ) -> Self {
        Self {
            event_id: event_id.into(),
            subject: subject.into(),
            class: class.into(),
            direction: 0.0,
            weight: 1.0,
            description: String::new(),
            occurred_at,
            known_at: known_at.max(occurred_at),
        }
    }

    pub fn with_direction(mut self, direction: f64) -> Self {
        self.direction = direction.clamp(-1.0, 1.0);
        self
    }

    pub fn with_weight(mut self, weight: f64) -> Self {
        self.weight = weight.clamp(0.0, 1.0);
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Re-target the event at another subject, keeping both times.
    ///
    /// A macro release lands on a series id; the caller who knows which
    /// instruments transmit it re-targets a copy per instrument.
    pub fn for_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    /// When the event was true in the world.
    pub fn occurred_at(&self) -> Timestamp {
        self.occurred_at
    }

    /// When this platform could first have acted on it. Never earlier than
    /// [`Self::occurred_at`].
    pub fn known_at(&self) -> Timestamp {
        self.known_at
    }

    /// One event per resolved primary entity of a news item.
    ///
    /// The known-time is the ingestion time clamped forward to publication: a
    /// story cannot have been actionable before it was published, however the
    /// feed stamped it. Unresolved mentions produce nothing — an event that
    /// cannot be attached to an instrument cannot explain its moves.
    pub fn from_news(item: &NewsItem) -> Vec<Self> {
        let class = item
            .topics
            .first()
            .cloned()
            .unwrap_or_else(|| format!("news/{}", item.source.as_str()));
        item.primary_entities()
            .into_iter()
            .filter_map(|mention| {
                let entity = mention.entity_id.as_ref()?;
                let sentiment = mention.sentiment.unwrap_or(item.sentiment);
                Some(
                    Self::new(
                        format!("{}:{entity}", item.item_id),
                        entity.clone(),
                        class.clone(),
                        item.published_at,
                        item.provenance.ingestion_time,
                    )
                    .with_direction(sentiment.effective())
                    .with_weight(item.evidential_weight())
                    .with_description(item.headline.clone()),
                )
            })
            .collect()
    }

    /// A reported fundamental as an event on its entity.
    ///
    /// Valid-time is the period end; known-time is the ingestion time, clamped
    /// forward — a figure about a quarter is knowable only once filed.
    pub fn from_fundamental(update: &FundamentalUpdate) -> Self {
        Self::new(
            format!(
                "{}:{}:{}",
                update.entity_id,
                update.metric,
                update.period_end.as_nanos()
            ),
            update.entity_id.clone(),
            format!("fundamental/{}", update.metric),
            update.period_end,
            update.provenance.ingestion_time,
        )
        .with_direction(update.surprise().map(f64::signum).unwrap_or(0.0))
        .with_weight(update.quality.score())
        .with_description(format!(
            "{} reported for {}",
            update.metric, update.entity_id
        ))
    }

    /// A macro release as an event on its series.
    ///
    /// The subject is the series id; use [`Self::for_subject`] to land copies
    /// on the instruments that transmit it.
    pub fn from_macro(observation: &MacroObservation) -> Self {
        Self::new(
            format!(
                "{}:{}",
                observation.series_id,
                observation.reference_date.as_nanos()
            ),
            observation.series_id.clone(),
            format!("macro/{}", observation.series_id),
            observation.reference_date,
            observation.provenance.ingestion_time,
        )
        .with_direction(observation.surprise().map(f64::signum).unwrap_or(0.0))
        .with_weight(observation.quality.score())
        .with_description(format!(
            "{} released at {} {}",
            observation.series_id, observation.value, observation.unit
        ))
    }
}

/// Events filtered by known-time, the only form a detection context carries.
///
/// The point of the type is what it makes impossible: the sole admitting
/// constructor, [`KnownEvents::known_by`], drops any event not knowable by the
/// stated time, so a caller physically cannot hand the detector an event from
/// the future of the scan. The container also remembers *through when* the
/// event stream was watched — an empty set with coverage means "we looked and
/// there was nothing", while [`KnownEvents::none`] means "nobody looked", and
/// only the former supports calling a move unexplained.
#[derive(Clone, Debug, Default)]
pub struct KnownEvents {
    coverage: Option<Timestamp>,
    events: Vec<MarketEvent>,
}

impl KnownEvents {
    /// No event visibility at all. Distinct from an empty filtered set.
    pub fn none() -> Self {
        Self::default()
    }

    /// Admit events knowable at `known_by`; anything later is dropped, not
    /// deferred. Events are kept sorted by known-time then id, so everything
    /// downstream is deterministic.
    pub fn known_by(known_by: Timestamp, events: Vec<MarketEvent>) -> Self {
        let mut kept: Vec<MarketEvent> = events
            .into_iter()
            .filter(|event| event.known_at <= known_by)
            .collect();
        kept.sort_by(|a, b| {
            a.known_at
                .cmp(&b.known_at)
                .then_with(|| a.event_id.cmp(&b.event_id))
        });
        Self {
            coverage: Some(known_by),
            events: kept,
        }
    }

    /// Through when the event stream was watched, if it was watched at all.
    pub fn coverage(&self) -> Option<Timestamp> {
        self.coverage
    }

    /// Whether the stream was watched through `at` — the precondition for
    /// claiming that no catalyst existed.
    pub fn covers(&self, at: Timestamp) -> bool {
        self.coverage.is_some_and(|c| c >= at)
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, MarketEvent> {
        self.events.iter()
    }

    /// Events landing on one subject, in known-time order.
    pub fn for_subject<'a>(
        &'a self,
        subject: &'a str,
    ) -> impl Iterator<Item = &'a MarketEvent> + 'a {
        self.events.iter().filter(move |e| e.subject == subject)
    }
}

/// Which history an estimate was drawn from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImpactScope {
    /// Enough history on this very instrument.
    Instrument,
    /// Fell back to the class across all instruments.
    Class,
}

/// What an event class has historically done. Every field is a statistic over
/// recorded event→move outcomes; nothing here is a price or a size.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImpactEstimate {
    pub scope: ImpactScope,
    /// Outcomes behind the estimate.
    pub observations: usize,
    /// Mean absolute log return that followed events of the class.
    pub mean_abs_return: f64,
    /// Median absolute log return — the "typically moves it X%" number.
    pub median_abs_return: f64,
    /// Mean signed log return, carrying the class's directional tendency.
    pub mean_signed_return: f64,
    /// Fraction of events followed by a move of at least `materiality`.
    pub hit_rate: f64,
    /// The materiality threshold the hit rate was measured against, as an
    /// absolute log return.
    pub materiality: f64,
    /// Median known-time-to-move lag.
    pub typical_lag: Duration,
}

impl ImpactEstimate {
    /// The sentence REASON gets instead of a headline.
    pub fn statement(&self) -> String {
        format!(
            "historically moves it {:.1}% (median; mean {:.1}%) about {:?} after becoming known, \
             exceeding {:.1}% in {:.0}% of {} prior event(s){}",
            self.median_abs_return * 100.0,
            self.mean_abs_return * 100.0,
            self.typical_lag,
            self.materiality * 100.0,
            self.hit_rate * 100.0,
            self.observations,
            match self.scope {
                ImpactScope::Instrument => "",
                ImpactScope::Class => ", class-wide",
            }
        )
    }
}

/// The answer to "what does this class of event do to this instrument?".
///
/// An enum rather than an `Option` so that refusing to answer is itself a
/// first-class, serializable answer that survives into the anomaly detail.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ImpactAssessment {
    /// Fewer recorded outcomes than the stated minimum. Not zero impact —
    /// unknown impact, and the two must never be conflated.
    InsufficientHistory {
        observations: usize,
        required: usize,
    },
    Estimated(ImpactEstimate),
}

impl ImpactAssessment {
    pub fn is_estimated(&self) -> bool {
        matches!(self, Self::Estimated(_))
    }

    pub fn estimate(&self) -> Option<&ImpactEstimate> {
        match self {
            Self::Estimated(estimate) => Some(estimate),
            Self::InsufficientHistory { .. } => None,
        }
    }

    /// Prose for anomaly descriptions. The insufficient case says so plainly
    /// rather than quoting a number that does not exist.
    pub fn statement(&self) -> String {
        match self {
            Self::Estimated(estimate) => estimate.statement(),
            Self::InsufficientHistory {
                observations,
                required,
            } => format!(
                "insufficient history to state a typical impact \
                 ({observations} prior event(s) recorded, {required} required)"
            ),
        }
    }
}

/// One accepted event→move outcome.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct ImpactObservation {
    signed_return: f64,
    lag_nanos: i64,
}

/// What events of each class have historically done to each instrument.
///
/// Fed by the caller from replayed or accumulated history via
/// [`ImpactHistory::record_outcome`], which is the gate where the temporal
/// discipline is enforced: a pair whose move began before the event was
/// knowable is refused, so no statistic can ever have look-ahead baked in.
#[derive(Clone, Debug)]
pub struct ImpactHistory {
    minimum_observations: usize,
    materiality: f64,
    /// class → subject → outcomes. `BTreeMap`s keep every aggregate
    /// deterministic.
    outcomes: BTreeMap<String, BTreeMap<String, Vec<ImpactObservation>>>,
}

impl Default for ImpactHistory {
    fn default() -> Self {
        // Eight observations is the least a median and a hit rate mean
        // anything; two percent is a materially tradable daily move.
        Self::new(8, 0.02)
    }
}

impl ImpactHistory {
    pub fn new(minimum_observations: usize, materiality: f64) -> Self {
        Self {
            minimum_observations: minimum_observations.max(1),
            materiality: materiality.abs(),
            outcomes: BTreeMap::new(),
        }
    }

    /// Observations below which the history refuses to state an impact.
    pub fn minimum_observations(&self) -> usize {
        self.minimum_observations
    }

    /// Absolute log return counted as a hit by the hit rate.
    pub fn materiality(&self) -> f64 {
        self.materiality
    }

    /// Outcomes recorded for a class, across all subjects.
    pub fn observations_of(&self, class: &str) -> usize {
        self.outcomes
            .get(class)
            .map(|by_subject| by_subject.values().map(Vec::len).sum())
            .unwrap_or(0)
    }

    /// Record what followed an event. Returns whether the pair was accepted.
    ///
    /// A move that began before the event was knowable is refused outright:
    /// recording it would launder look-ahead into a statistic, which is worse
    /// than the look-ahead itself because the statistic looks clean.
    pub fn record_outcome(
        &mut self,
        event: &MarketEvent,
        move_begun_at: Timestamp,
        signed_return: f64,
    ) -> bool {
        if move_begun_at < event.known_at() || !signed_return.is_finite() {
            return false;
        }
        self.outcomes
            .entry(event.class.clone())
            .or_default()
            .entry(event.subject.clone())
            .or_default()
            .push(ImpactObservation {
                signed_return,
                lag_nanos: move_begun_at.since(event.known_at()).as_nanos(),
            });
        true
    }

    /// What an event of `class` does to `subject`: the instrument's own
    /// history when there is enough of it, the class across all instruments
    /// otherwise, and a refusal when even that is too thin.
    pub fn assess(&self, class: &str, subject: &str) -> ImpactAssessment {
        let by_subject = self.outcomes.get(class);
        let own: &[ImpactObservation] = by_subject
            .and_then(|m| m.get(subject))
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if own.len() >= self.minimum_observations {
            return ImpactAssessment::Estimated(self.estimate(own, ImpactScope::Instrument));
        }
        let class_wide: Vec<ImpactObservation> = by_subject
            .map(|m| m.values().flatten().copied().collect())
            .unwrap_or_default();
        if class_wide.len() >= self.minimum_observations {
            return ImpactAssessment::Estimated(self.estimate(&class_wide, ImpactScope::Class));
        }
        ImpactAssessment::InsufficientHistory {
            observations: class_wide.len(),
            required: self.minimum_observations,
        }
    }

    fn estimate(&self, observations: &[ImpactObservation], scope: ImpactScope) -> ImpactEstimate {
        let absolute: Vec<f64> = observations.iter().map(|o| o.signed_return.abs()).collect();
        let signed: Vec<f64> = observations.iter().map(|o| o.signed_return).collect();
        let lags: Vec<f64> = observations.iter().map(|o| o.lag_nanos as f64).collect();
        let hits = absolute.iter().filter(|a| **a >= self.materiality).count();
        ImpactEstimate {
            scope,
            observations: observations.len(),
            mean_abs_return: stats::mean(&absolute),
            median_abs_return: stats::median(&absolute),
            mean_signed_return: stats::mean(&signed),
            hit_rate: hits as f64 / absolute.len() as f64,
            materiality: self.materiality,
            typical_lag: Duration::from_nanos(stats::median(&lags) as i64),
        }
    }
}

/// The structured linkage an anomaly carries when it names an event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CatalystLink {
    pub event_id: String,
    pub event_class: String,
    /// When the event became knowable — by construction never after the move
    /// it explains begins.
    pub event_known_at: Timestamp,
    /// For an explained move: known-time to the start of the move window. For
    /// a landing: known-time to the scan, i.e. the event's age.
    pub lag: Duration,
    /// What events of this class have historically done — or the recorded
    /// refusal to say.
    pub impact: ImpactAssessment,
}

/// Links moves to knowable prior events and events to historical impact.
///
/// Consumes the context's price series and its [`KnownEvents`], and emits
/// [`AnomalyKind::Catalyst`] for an explained move or a landing with estimable
/// impact, and [`AnomalyKind::UnexplainedMove`] for a large move nothing
/// knowable explains — but only when the event stream was actually watched
/// through the move window, because "no events supplied" is not "no catalyst
/// existed".
#[derive(Clone, Debug)]
pub struct CatalystDetector {
    /// Robust window the move z-score is measured against, matching the
    /// return-anomaly detector so the two agree on what "a move" is.
    pub window: usize,
    /// Robust sigma at which a return counts as a move — and at which a
    /// landing's expected move is worth reporting.
    pub move_threshold: f64,
    /// Oldest a knowable event may be and still explain a move. A filing from
    /// a month ago does not explain this morning's gap.
    pub max_explanation_lag: Duration,
    /// How recently an event must have become known to count as landing now.
    pub landing_lookback: Duration,
    /// What event classes have historically done, fed by the caller.
    pub history: ImpactHistory,
}

impl Default for CatalystDetector {
    fn default() -> Self {
        Self {
            window: 60,
            move_threshold: 3.0,
            max_explanation_lag: Duration::from_days(3),
            landing_lookback: Duration::from_days(1),
            history: ImpactHistory::default(),
        }
    }
}

impl CatalystDetector {
    pub fn with_history(history: ImpactHistory) -> Self {
        Self {
            history,
            ..Self::default()
        }
    }

    /// The best explanation among knowable candidates: most recently knowable
    /// first (the shortest lag is the most plausible link), then heaviest
    /// source, then smallest id so ties are deterministic.
    fn best_explanation<'a>(
        &self,
        context: &'a DetectionContext,
        subject: &'a str,
        move_begin: Timestamp,
    ) -> Option<&'a MarketEvent> {
        context
            .events
            .for_subject(subject)
            .filter(|event| event.known_at() < move_begin)
            .filter(|event| move_begin.since(event.known_at()) <= self.max_explanation_lag)
            .max_by(|a, b| {
                a.known_at()
                    .cmp(&b.known_at())
                    .then_with(|| {
                        a.weight
                            .partial_cmp(&b.weight)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .then_with(|| b.event_id.cmp(&a.event_id))
            })
    }
}

impl Detector for CatalystDetector {
    fn name(&self) -> &str {
        "catalyst"
    }

    fn threshold(&self) -> f64 {
        self.move_threshold
    }

    fn expected_false_positive_rate(&self) -> f64 {
        // Fires on the same fat-tailed three-sigma moves as the return
        // detector, but only when the context carries event visibility, and
        // landings are additionally gated by recorded history.
        5.0
    }

    fn detect(&self, context: &DetectionContext) -> Vec<Anomaly> {
        let mut anomalies = Vec::new();
        if context.bar_interval <= Duration::ZERO {
            // Without a bar interval the move window has no start, and every
            // temporal comparison below would be a guess.
            return anomalies;
        }
        let move_begin = context.as_of.saturating_sub(context.bar_interval);
        let landing_from = context.as_of.saturating_sub(self.landing_lookback);

        for (subject, prices) in &context.prices {
            if prices.len() < self.window + 2 {
                continue;
            }
            let returns = stats::log_returns(prices);
            let history =
                &returns[returns.len().saturating_sub(self.window + 1)..returns.len() - 1];
            let latest = returns[returns.len() - 1];
            if history.len() < 10 {
                continue;
            }
            let centre = stats::median(history);
            let scale = stats::median_absolute_deviation(history);
            if scale <= 1e-12 {
                continue;
            }
            let z = (latest - centre) / scale;

            // Events already cited as an explanation this scan; a landing for
            // the same event would state the same story twice.
            let mut explained: BTreeSet<&str> = BTreeSet::new();

            if z.abs() >= self.move_threshold {
                match self.best_explanation(context, subject, move_begin) {
                    Some(event) => {
                        let lag = move_begin.since(event.known_at());
                        let impact = self.history.assess(&event.class, subject);
                        anomalies.push(Anomaly {
                            kind: AnomalyKind::Catalyst,
                            subject: subject.clone(),
                            detector: self.name().to_string(),
                            z_score: z,
                            observed: latest,
                            expected: centre,
                            sample_size: history.len(),
                            detected_at: context.as_of,
                            description: format!(
                                "{subject} returned {:+.2}% ({z:+.1} robust sigma), plausibly \
                                 explained by {} \"{}\" known {lag:?} before the move; {}",
                                latest * 100.0,
                                event.class,
                                event.description,
                                impact.statement()
                            ),
                            catalyst: Some(CatalystLink {
                                event_id: event.event_id.clone(),
                                event_class: event.class.clone(),
                                event_known_at: event.known_at(),
                                lag,
                                impact,
                            }),
                        });
                        explained.insert(event.event_id.as_str());
                    }
                    None if context.events.covers(move_begin) => {
                        let considered = context.events.for_subject(subject).count();
                        anomalies.push(Anomaly {
                            kind: AnomalyKind::UnexplainedMove,
                            subject: subject.clone(),
                            detector: self.name().to_string(),
                            z_score: z,
                            observed: latest,
                            expected: centre,
                            sample_size: history.len(),
                            detected_at: context.as_of,
                            description: format!(
                                "{subject} returned {:+.2}% ({z:+.1} robust sigma) with no \
                                 knowable catalyst in the prior {:?}; {considered} event(s) on \
                                 the name were considered",
                                latest * 100.0,
                                self.max_explanation_lag
                            ),
                            catalyst: None,
                        });
                    }
                    // Without event coverage through the move window, "no
                    // catalyst found" is a statement about our inputs, not
                    // about the world, and it is not emitted.
                    None => {}
                }
            }

            // Landing pass: events that just became known, with what their
            // class has historically done. A class the history cannot speak
            // for produces nothing — the event itself is already visible to
            // the platform through the news path; this detector's only added
            // value is the impact estimate, and it refuses to invent one.
            for event in context.events.for_subject(subject) {
                if event.known_at() <= landing_from
                    || event.known_at() > context.as_of
                    || explained.contains(event.event_id.as_str())
                {
                    continue;
                }
                let ImpactAssessment::Estimated(estimate) =
                    self.history.assess(&event.class, subject)
                else {
                    continue;
                };
                let sign = if event.direction < 0.0 {
                    -1.0
                } else if event.direction > 0.0 {
                    1.0
                } else if estimate.mean_signed_return < 0.0 {
                    -1.0
                } else {
                    1.0
                };
                let expected_z = sign * estimate.median_abs_return / scale;
                if expected_z.abs() < self.move_threshold {
                    // A class that historically moves the name within its
                    // normal noise is not a catalyst worth escalating.
                    continue;
                }
                anomalies.push(Anomaly {
                    kind: AnomalyKind::Catalyst,
                    subject: subject.clone(),
                    detector: self.name().to_string(),
                    z_score: expected_z,
                    observed: estimate.median_abs_return,
                    expected: scale,
                    sample_size: estimate.observations,
                    detected_at: context.as_of,
                    description: format!(
                        "{} \"{}\" landed on {subject}: {}",
                        event.class,
                        event.description,
                        estimate.statement()
                    ),
                    catalyst: Some(CatalystLink {
                        event_id: event.event_id.clone(),
                        event_class: event.class.clone(),
                        event_known_at: event.known_at(),
                        lag: context.as_of.since(event.known_at()),
                        impact: ImpactAssessment::Estimated(estimate),
                    }),
                });
            }
        }
        anomalies
    }
}
