//! Non-market intelligence inputs: news, fundamentals, macro and alternative
//! data.
//!
//! These are the records the Deep Brain reasons over. They live beside the
//! financial object model rather than inside an ingestion service because both
//! producers and consumers need the vocabulary: an adapter emits them, the
//! world model absorbs them, and the reasoning engine cites them as evidence.
//!
//! Every record carries [`crate::quality::Provenance`], so a thesis can always
//! be traced back to the exact source and timestamp of what it rests on.

use qip_core::{Decimal, Timestamp};
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};

use crate::quality::{DataQuality, Provenance};

/// Directional reading of a piece of text, in `[-1, 1]`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct Sentiment {
    /// Negative through positive.
    pub polarity: f64,
    /// How confident the scorer is, in `[0, 1]`.
    pub confidence: f64,
    /// Whether the item is a surprise relative to expectations, in `[0, 1]`.
    pub novelty: f64,
}

impl Sentiment {
    pub fn neutral() -> Self {
        Self {
            polarity: 0.0,
            confidence: 0.5,
            novelty: 0.0,
        }
    }

    /// Polarity weighted by confidence — what downstream models should use, so
    /// a confident mild reading outranks an uncertain extreme one.
    pub fn effective(&self) -> f64 {
        self.polarity.clamp(-1.0, 1.0) * self.confidence.clamp(0.0, 1.0)
    }

    pub fn is_material(&self) -> bool {
        self.effective().abs() > 0.3 && self.novelty > 0.3
    }
}

/// An entity named in a document, with where and how strongly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityMention {
    /// Surface form as it appeared in the text.
    pub text: String,
    /// Resolved canonical entity, once identity resolution has run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_id: Option<String>,
    /// Confidence in the resolution, in `[0, 1]`.
    pub confidence: f64,
    /// Whether the entity is the subject of the document or merely referenced.
    pub is_primary: bool,
    /// Sentiment directed at this entity specifically, which can differ from
    /// the document's overall tone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sentiment: Option<Sentiment>,
}

/// Where a document came from, which governs how much weight it earns.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NewsSource {
    /// Statutory filing: the highest-reliability source.
    RegulatoryFiling,
    /// Company press release or investor communication.
    CompanyAnnouncement,
    /// Central bank or government statistical release.
    OfficialStatistics,
    /// Established newswire.
    Newswire,
    /// Sell-side or independent research.
    Research,
    /// Earnings call or investor presentation transcript.
    Transcript,
    /// Social or forum content.
    Social,
}

impl NewsSource {
    /// Prior weight on the source's reliability, in `[0, 1]`.
    ///
    /// Used as the starting confidence for evidence drawn from it. A filing is
    /// a legal statement; a forum post is a rumour. The reasoning engine must
    /// not treat them alike.
    pub fn reliability(&self) -> f64 {
        match self {
            Self::RegulatoryFiling => 0.98,
            Self::OfficialStatistics => 0.97,
            Self::CompanyAnnouncement => 0.9,
            Self::Transcript => 0.88,
            Self::Newswire => 0.82,
            Self::Research => 0.7,
            Self::Social => 0.25,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RegulatoryFiling => "regulatory_filing",
            Self::CompanyAnnouncement => "company_announcement",
            Self::OfficialStatistics => "official_statistics",
            Self::Newswire => "newswire",
            Self::Research => "research",
            Self::Transcript => "transcript",
            Self::Social => "social",
        }
    }
}

/// A textual item: filing, release, wire story, transcript.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewsItem {
    pub item_id: String,
    pub headline: String,
    pub body: String,
    pub source: NewsSource,
    /// When the item was published.
    pub published_at: Timestamp,
    pub entities: Vec<EntityMention>,
    pub sentiment: Sentiment,
    /// Free-form topic tags, e.g. `guidance`, `m&a`, `monetary_policy`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topics: Vec<String>,
    pub provenance: Provenance,
    #[serde(default)]
    pub quality: DataQuality,
}

impl NewsItem {
    /// Entities the item is actually about, as opposed to those merely named.
    pub fn primary_entities(&self) -> Vec<&EntityMention> {
        self.entities.iter().filter(|e| e.is_primary).collect()
    }

    /// How much weight this item deserves as evidence: source reliability,
    /// discounted by data quality and by how confidently entities resolved.
    pub fn evidential_weight(&self) -> f64 {
        let resolution = if self.entities.is_empty() {
            0.5
        } else {
            self.entities.iter().map(|e| e.confidence).sum::<f64>() / self.entities.len() as f64
        };
        self.source.reliability() * self.quality.score() * resolution
    }
}

impl EventBody for NewsItem {
    const TOPIC: Topic = Topic::NewsReceived;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{}:{}", self.provenance.source, self.item_id))
    }
}

/// A reported company fundamental.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FundamentalUpdate {
    /// Canonical entity the figure belongs to.
    pub entity_id: String,
    /// Metric name, e.g. `revenue`, `eps_diluted`, `free_cash_flow`.
    pub metric: String,
    pub value: Decimal,
    /// Unit or currency code the value is expressed in.
    pub unit: String,
    /// End of the period the figure covers.
    pub period_end: Timestamp,
    pub period: FiscalPeriod,
    /// Consensus expectation, where one existed before the release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consensus: Option<Decimal>,
    /// Same metric for the prior comparable period.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_value: Option<Decimal>,
    /// True when the figure restates a previously reported one.
    #[serde(default)]
    pub is_restatement: bool,
    pub provenance: Provenance,
    #[serde(default)]
    pub quality: DataQuality,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FiscalPeriod {
    Quarter,
    HalfYear,
    Year,
    TrailingTwelveMonths,
}

impl FundamentalUpdate {
    /// Surprise against consensus, as a fraction of the expectation.
    ///
    /// `None` when there was no consensus — an absent expectation is not a
    /// zero surprise, and conflating the two manufactures signal.
    pub fn surprise(&self) -> Option<f64> {
        let consensus = self.consensus?;
        if consensus.is_zero() {
            return None;
        }
        Some((self.value - consensus).to_f64() / consensus.abs().to_f64())
    }

    /// Growth against the prior comparable period.
    pub fn growth(&self) -> Option<f64> {
        let prior = self.prior_value?;
        if prior.is_zero() {
            return None;
        }
        Some((self.value - prior).to_f64() / prior.abs().to_f64())
    }

    /// Whether the surprise is large enough to be worth investigating.
    pub fn is_significant_surprise(&self, threshold: f64) -> bool {
        self.surprise().is_some_and(|s| s.abs() >= threshold)
    }
}

impl EventBody for FundamentalUpdate {
    const TOPIC: Topic = Topic::FundamentalUpdated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "{}:{}:{}:{}",
            self.entity_id,
            self.metric,
            self.period_end.as_nanos(),
            u8::from(self.is_restatement)
        ))
    }
}

/// A macroeconomic observation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MacroObservation {
    /// Series identifier, e.g. `US.CPI.YOY`, `EA.POLICY_RATE`.
    pub series_id: String,
    /// ISO country or region code.
    pub region: String,
    pub value: f64,
    pub unit: String,
    /// The date the observation refers to.
    pub reference_date: Timestamp,
    /// Consensus forecast ahead of the release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consensus: Option<f64>,
    /// Previously published value for the prior period.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<f64>,
    /// True when this revises an already-published figure.
    ///
    /// Revisions are the classic point-in-time trap: a backtest that reads the
    /// revised value is reading data that did not exist at decision time.
    #[serde(default)]
    pub is_revision: bool,
    pub provenance: Provenance,
    #[serde(default)]
    pub quality: DataQuality,
}

impl MacroObservation {
    /// Surprise against consensus, in the series' own units.
    pub fn surprise(&self) -> Option<f64> {
        self.consensus.map(|c| self.value - c)
    }

    /// Change against the previous published figure.
    pub fn change(&self) -> Option<f64> {
        self.previous.map(|p| self.value - p)
    }
}

impl EventBody for MacroObservation {
    const TOPIC: Topic = Topic::MacroUpdated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "{}:{}:{}",
            self.series_id,
            self.reference_date.as_nanos(),
            self.provenance.ingestion_time.as_nanos()
        ))
    }
}

/// A reading from a non-traditional dataset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlternativeDataPoint {
    /// Dataset name, e.g. `satellite.parking_lot_counts`.
    pub dataset: String,
    /// Entity or region the reading concerns.
    pub subject_id: String,
    /// Metric within the dataset.
    pub metric: String,
    pub value: f64,
    pub unit: String,
    /// When the underlying phenomenon was observed.
    pub observed_at: Timestamp,
    /// Typical lead over the reported fundamental this proxies for, in days.
    ///
    /// The reason alternative data is bought at all: it is informative before
    /// the official number lands.
    #[serde(default)]
    pub lead_days: f64,
    /// Historical correlation with the fundamental it proxies for.
    #[serde(default)]
    pub proxy_correlation: f64,
    /// What official metric this is a proxy for, when it is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxies_for: Option<String>,
    pub provenance: Provenance,
    #[serde(default)]
    pub quality: DataQuality,
}

impl AlternativeDataPoint {
    /// Whether the reading is strong enough a proxy to act on.
    ///
    /// Alternative data is noisy and expensive; a weak proxy is worse than
    /// nothing because it looks like information.
    pub fn is_actionable(&self) -> bool {
        self.proxy_correlation.abs() >= 0.4
            && self.quality.meets(crate::quality::DECISION_QUALITY_FLOOR)
            && self.proxies_for.is_some()
    }
}

impl EventBody for AlternativeDataPoint {
    const TOPIC: Topic = Topic::AlternativeDataReceived;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "{}:{}:{}:{}",
            self.dataset,
            self.subject_id,
            self.metric,
            self.observed_at.as_nanos()
        ))
    }
}

/// A change to instrument reference data.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReferenceDataUpdate {
    pub object_id: String,
    /// Field that changed, e.g. `lot_size`, `sector`, `venue`.
    pub field: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_value: Option<String>,
    pub new_value: String,
    pub effective_from: Timestamp,
    pub provenance: Provenance,
}

impl EventBody for ReferenceDataUpdate {
    const TOPIC: Topic = Topic::ReferenceDataUpdated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "{}:{}:{}",
            self.object_id,
            self.field,
            self.effective_from.as_nanos()
        ))
    }
}

/// Emitted when a record fails validation, so bad data is visible rather than
/// silently dropped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataQualityFailure {
    /// Adapter that produced the offending record.
    pub source: String,
    /// Topic the record would have been published on.
    pub intended_topic: String,
    /// Subject of the record, where identifiable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    pub issues: Vec<String>,
    pub detected_at: Timestamp,
    /// Whether the record was rejected outright or admitted with a lower score.
    pub rejected: bool,
}

impl EventBody for DataQualityFailure {
    const TOPIC: Topic = Topic::DataQualityFailed;
    const SCHEMA_VERSION: u32 = 1;
}
