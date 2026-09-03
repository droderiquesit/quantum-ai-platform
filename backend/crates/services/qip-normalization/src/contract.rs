//! Declarative data contracts.
//!
//! A contract states what a well-formed record looks like: which fields must be
//! present, what ranges they may take, how stale they may be. Providers change
//! units, drop fields and start sending nulls without warning; asserting the
//! contract at the boundary turns that into a loud failure instead of a quiet
//! corruption.

use qip_core::{Duration, Timestamp};
use qip_events::Topic;
use qip_financial::quality::DataQuality;
use qip_market_ingestion::adapter::SensedRecord;
use serde::{Deserialize, Serialize};

/// A rule over one numeric field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldRule {
    pub field: String,
    /// Inclusive lower bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<f64>,
    /// Inclusive upper bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<f64>,
    /// Whether the field must be present at all.
    pub required: bool,
}

impl FieldRule {
    pub fn required(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            minimum: None,
            maximum: None,
            required: true,
        }
    }

    pub fn between(field: impl Into<String>, minimum: f64, maximum: f64) -> Self {
        Self {
            field: field.into(),
            minimum: Some(minimum),
            maximum: Some(maximum),
            required: true,
        }
    }

    pub fn positive(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            minimum: Some(f64::MIN_POSITIVE),
            maximum: None,
            required: true,
        }
    }

    /// Check a value against the rule.
    pub fn check(&self, value: Option<f64>) -> Option<String> {
        match value {
            None if self.required => Some(format!("{} is missing", self.field)),
            None => None,
            Some(v) if !v.is_finite() => Some(format!("{} is not finite", self.field)),
            Some(v) => {
                if let Some(minimum) = self.minimum
                    && v < minimum
                {
                    return Some(format!(
                        "{} = {v} is below the minimum {minimum}",
                        self.field
                    ));
                }
                if let Some(maximum) = self.maximum
                    && v > maximum
                {
                    return Some(format!(
                        "{} = {v} is above the maximum {maximum}",
                        self.field
                    ));
                }
                None
            }
        }
    }
}

/// A single failure to meet a contract.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ContractViolation {
    pub contract: String,
    pub topic: String,
    pub subject: String,
    pub detail: String,
}

/// Outcome of checking a record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum RuleOutcome {
    Passed,
    Failed(Vec<ContractViolation>),
}

impl RuleOutcome {
    pub fn is_passed(&self) -> bool {
        matches!(self, Self::Passed)
    }

    pub fn violations(&self) -> &[ContractViolation] {
        match self {
            Self::Passed => &[],
            Self::Failed(v) => v,
        }
    }
}

/// The expectations for one topic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DataContract {
    pub name: String,
    pub topic: Topic,
    pub rules: Vec<FieldRule>,
    /// How stale a record may be before it is a violation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_staleness: Option<Duration>,
    /// Minimum acceptable data-quality score.
    #[serde(default)]
    pub minimum_quality: f64,
}

impl DataContract {
    /// Check a record against this contract.
    pub fn check(&self, record: &SensedRecord, now: Timestamp) -> RuleOutcome {
        if record.topic() != self.topic {
            return RuleOutcome::Passed;
        }
        let mut violations = Vec::new();
        let values = extract_fields(record);

        for rule in &self.rules {
            let value = values
                .iter()
                .find(|(name, _)| name == &rule.field)
                .map(|(_, v)| *v);
            if let Some(detail) = rule.check(value) {
                violations.push(ContractViolation {
                    contract: self.name.clone(),
                    topic: self.topic.name().to_string(),
                    subject: record.subject(),
                    detail,
                });
            }
        }

        if let Some(limit) = self.max_staleness {
            let age = now.since(record.occurred_at());
            if age > limit {
                violations.push(ContractViolation {
                    contract: self.name.clone(),
                    topic: self.topic.name().to_string(),
                    subject: record.subject(),
                    detail: format!("record is {age:?} old, limit is {limit:?}"),
                });
            }
        }

        // The bug this prevents: `minimum_quality` was carried on every
        // standard contract and documented as "minimum acceptable
        // data-quality score", but nothing ever read it back against the
        // record's own `DataQuality` — a limit that cannot fire reads as
        // protection and is not (the same defect the risk domain names for
        // `MaxExpectedShortfall`). A record whose completeness or confidence
        // had collapsed to near zero still passed every contract as long as
        // its numeric fields happened to sit inside range, because quality
        // was never compared to the floor the contract itself declared.
        if self.minimum_quality > 0.0
            && let Some(quality) = record_quality(record)
            && !quality.meets(self.minimum_quality)
        {
            violations.push(ContractViolation {
                contract: self.name.clone(),
                topic: self.topic.name().to_string(),
                subject: record.subject(),
                detail: format!(
                    "data-quality score {:.3} is below the minimum {:.3}",
                    quality.score(),
                    self.minimum_quality
                ),
            });
        }

        if violations.is_empty() {
            RuleOutcome::Passed
        } else {
            RuleOutcome::Failed(violations)
        }
    }

    /// The contracts the platform ships with.
    ///
    /// Bounds are wide on purpose: they catch unit errors and corrupt feeds,
    /// not unusual-but-real markets. A contract that fires on a genuine 20%
    /// move would be turned off within a week and protect nothing.
    pub fn standard() -> Vec<Self> {
        vec![
            Self {
                name: "quote-sanity".into(),
                topic: Topic::MarketQuote,
                rules: vec![
                    FieldRule::positive("bid"),
                    FieldRule::positive("ask"),
                    FieldRule::between("spread_bps", 0.0, 5_000.0),
                    FieldRule {
                        field: "bid_size".into(),
                        minimum: Some(0.0),
                        maximum: None,
                        required: true,
                    },
                    FieldRule {
                        field: "ask_size".into(),
                        minimum: Some(0.0),
                        maximum: None,
                        required: true,
                    },
                ],
                max_staleness: Some(Duration::from_mins(30)),
                minimum_quality: 0.5,
            },
            Self {
                name: "trade-sanity".into(),
                topic: Topic::MarketTrade,
                rules: vec![
                    FieldRule::positive("price"),
                    FieldRule {
                        field: "size".into(),
                        minimum: Some(0.0),
                        maximum: None,
                        required: true,
                    },
                ],
                max_staleness: Some(Duration::from_mins(30)),
                minimum_quality: 0.5,
            },
            Self {
                name: "bar-sanity".into(),
                topic: Topic::MarketBar,
                rules: vec![
                    FieldRule::positive("open"),
                    FieldRule::positive("high"),
                    FieldRule::positive("low"),
                    FieldRule::positive("close"),
                    FieldRule {
                        field: "volume".into(),
                        minimum: Some(0.0),
                        maximum: None,
                        required: true,
                    },
                    // A single bar spanning more than a 3x range is a bad print
                    // or a missed corporate action, not a market move.
                    FieldRule::between("range_ratio", 0.0, 3.0),
                ],
                max_staleness: None,
                minimum_quality: 0.5,
            },
            Self {
                name: "macro-sanity".into(),
                topic: Topic::MacroUpdated,
                rules: vec![FieldRule::between("value", -1_000.0, 1_000.0)],
                max_staleness: None,
                minimum_quality: 0.7,
            },
            Self {
                name: "news-sanity".into(),
                topic: Topic::NewsReceived,
                rules: vec![
                    FieldRule::between("sentiment_polarity", -1.0, 1.0),
                    FieldRule::between("sentiment_confidence", 0.0, 1.0),
                ],
                max_staleness: None,
                minimum_quality: 0.3,
            },
        ]
    }
}

/// Extract the checkable numeric fields from a record.
fn extract_fields(record: &SensedRecord) -> Vec<(String, f64)> {
    match record {
        SensedRecord::Quote(q) => {
            let mut fields = vec![
                ("bid".to_string(), q.bid.to_f64()),
                ("ask".to_string(), q.ask.to_f64()),
                ("bid_size".to_string(), q.bid_size.to_f64()),
                ("ask_size".to_string(), q.ask_size.to_f64()),
            ];
            if let Some(bps) = q.spread_bps() {
                fields.push(("spread_bps".to_string(), bps));
            }
            fields
        }
        SensedRecord::Trade(t) => vec![
            ("price".to_string(), t.price.to_f64()),
            ("size".to_string(), t.size.to_f64()),
        ],
        SensedRecord::Tick(t) => vec![("price".to_string(), t.price.to_f64())],
        SensedRecord::Bar(b) => {
            let mut fields = vec![
                ("open".to_string(), b.open.to_f64()),
                ("high".to_string(), b.high.to_f64()),
                ("low".to_string(), b.low.to_f64()),
                ("close".to_string(), b.close.to_f64()),
                ("volume".to_string(), b.volume.to_f64()),
            ];
            if b.low.is_positive() {
                fields.push(("range_ratio".to_string(), b.high.to_f64() / b.low.to_f64()));
            }
            fields
        }
        SensedRecord::Macro(m) => vec![("value".to_string(), m.value)],
        SensedRecord::News(n) => vec![
            ("sentiment_polarity".to_string(), n.sentiment.polarity),
            ("sentiment_confidence".to_string(), n.sentiment.confidence),
        ],
        SensedRecord::AlternativeData(a) => vec![("value".to_string(), a.value)],
        _ => Vec::new(),
    }
}

/// The data-quality assessment attached to a record, when its kind carries
/// one. Reference-quality kinds (a corporate action, an order-book snapshot,
/// reference data) carry no `DataQuality` today, so a contract with a
/// `minimum_quality` on one of those topics has nothing to compare against
/// and is silently a no-op on that dimension — the numeric-field and
/// staleness rules still apply.
fn record_quality(record: &SensedRecord) -> Option<&DataQuality> {
    match record {
        SensedRecord::Quote(q) => Some(&q.quality),
        SensedRecord::Trade(t) => Some(&t.quality),
        SensedRecord::Tick(t) => Some(&t.quality),
        SensedRecord::Bar(b) => Some(&b.quality),
        SensedRecord::News(n) => Some(&n.quality),
        SensedRecord::Fundamental(f) => Some(&f.quality),
        SensedRecord::Macro(m) => Some(&m.quality),
        SensedRecord::AlternativeData(a) => Some(&a.quality),
        SensedRecord::Book(_)
        | SensedRecord::CorporateAction(_)
        | SensedRecord::ReferenceData(_) => None,
    }
}

/// Check a record against every applicable contract.
pub fn check_all(
    contracts: &[DataContract],
    record: &SensedRecord,
    now: Timestamp,
) -> Vec<ContractViolation> {
    contracts
        .iter()
        .flat_map(|c| c.check(record, now).violations().to_vec())
        .collect()
}
