//! Provider adapter ports.
//!
//! An adapter converts whatever a source publishes into [`SensedRecord`]s. It
//! does not decide what the record means, does not touch the event bus, and
//! does not hold platform state — that keeps a new provider to one small,
//! testable unit.

use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_events::bus::EventBus;
use qip_events::{EventBody, Topic};
use qip_financial::intelligence::{
    AlternativeDataPoint, DataQualityFailure, FundamentalUpdate, MacroObservation, NewsItem,
    ReferenceDataUpdate,
};
use qip_financial::quality::LicensingClass;
use qip_market::bar::Bar;
use qip_market::book::OrderBook;
use qip_market::corporate_action::CorporateAction;
use qip_market::quote::{Quote, Tick, Trade};
use serde::{Deserialize, Serialize};

/// The longest a record's own subject key may be, in characters.
///
/// A series id is an *identifier*, and in this platform an identifier is
/// permanent: it keys a feature-store series, it is written into the
/// hash-chained event log, and it is interpolated into the operator prose the
/// UNDERSTAND stage renders. The longest one this build mints is
/// `FX.{base}.{quote}` — eleven characters — and the vocabulary's
/// `{region}.{code}` is not much longer, so sixty-four is several times any
/// legitimate key and still far too small to be an allocation.
///
/// The failure is not hypothetical. A rate table answering with sixty-four
/// sixty-kilobyte currency codes produced 3,840,704 bytes of permanent key
/// from one poll, and nothing between the socket and the feature store
/// refused it: `max_events_per_batch` bounds the *number* of events, and
/// nothing bounded their size.
pub const MAX_SUBJECT_KEY_CHARS: usize = 64;

/// The largest magnitude a statistic may carry into the platform.
///
/// Not a judgement about what any particular series may plausibly print —
/// that belongs to the connector, which knows what its source publishes. This
/// is the arithmetic bound underneath every such judgement: the statistics
/// here are `f64` and second-moment statistics square their inputs. The
/// deepest feature series the world model retains is 512 values, so a variance
/// over a full series sums 512 squares, and that sum overflows to `inf` above
/// about `5.9e152` — `sqrt(f64::MAX / 512)`. At `1e150` the same sum is
/// `5.1e302` and finite, with two and a half decades still in hand for a
/// covariance or a longer window. Past the overflow point the sum is `inf`,
/// and `inf - inf` is `NaN`, whose comparisons answer `false` in both
/// directions — a limit check that neither passes nor fails. `1e300`, the
/// value that arrived through a rate table and was admitted end to end, is
/// a hundred and fifty decades past it.
///
/// Nothing legitimate is anywhere near this: world GDP expressed in yen is
/// about `6e14`, and the largest exchange rate the ECB has ever published is
/// about `1.8e6`.
///
/// Refused rather than clamped: a reading this large is not a measurement that
/// needs correcting, it is a measurement that never happened.
pub const MAX_STATISTIC_MAGNITUDE: f64 = 1e150;

/// A bounded, escaped rendering of text a vendor chose.
///
/// Every refusal below quotes the thing it refused, and the thing it refused
/// may be sixty kilobytes of a hostile response with an escape sequence in it.
/// The message travels into a quarantine entry, a `DataQualityFailure` on the
/// bus, the event log and an operator's terminal, so it carries at most the
/// first sixteen characters, `Debug`-escaped — which renders a newline as
/// `\n` and an ANSI introducer as `\u{1b}` — plus the length, which is the
/// part that actually tells an operator what happened.
pub(crate) fn bounded_excerpt(text: &str) -> String {
    const KEEP: usize = 16;
    let head: String = text.chars().take(KEEP).collect();
    if text.chars().nth(KEEP).is_some() {
        format!("{head:?}… ({} characters)", text.chars().count())
    } else {
        format!("{head:?}")
    }
}

/// Problems with a key a record will be stored and journalled under.
///
/// The character set is the one [`crate::narrative`] already holds a
/// configured series id to; the reason is different — that one keeps a
/// vendor's identifier from splitting a request line, this one keeps it from
/// becoming a permanent store key — and both refuse rather than sanitise,
/// because a key silently rewritten is a series nobody can find again.
fn subject_key_issues(kind: &str, key: &str) -> Vec<String> {
    if key.trim().is_empty() {
        return vec![format!(
            "an empty {kind}; a record with no subject cannot be read back under any key"
        )];
    }
    let length = key.chars().count();
    if length > MAX_SUBJECT_KEY_CHARS {
        return vec![format!(
            "the {kind} {} is {length} characters and the bound is {MAX_SUBJECT_KEY_CHARS}: a key \
             this long is not an identifier, and it would be permanent in the feature store and \
             in the event log",
            bounded_excerpt(key)
        )];
    }
    match key
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':')))
    {
        Some(offending) => vec![format!(
            "the {kind} {} contains {offending:?}: a subject key is ASCII letters, digits and \
             . - _ : only, because it is rendered into operator prose and into a request line \
             built by hand",
            bounded_excerpt(key)
        )],
        None => Vec::new(),
    }
}

/// Problems with a value that will be read as a statistic.
///
/// Finiteness was the whole of this check until a rate of `1e300` was shown to
/// travel from a response body to a `FeatureValue` untouched. Finite is not
/// the same as usable: see [`MAX_STATISTIC_MAGNITUDE`].
fn statistic_issues(kind: &str, subject: &str, value: f64) -> Vec<String> {
    if !value.is_finite() {
        return vec![format!(
            "non-finite {kind} for {}",
            bounded_excerpt(subject)
        )];
    }
    if value.abs() > MAX_STATISTIC_MAGNITUDE {
        return vec![format!(
            "the {kind} for {} is {value:e}, past the {MAX_STATISTIC_MAGNITUDE:e} beyond which a \
             second-moment statistic over this series overflows to infinity and every comparison \
             drawn from it answers false in both directions",
            bounded_excerpt(subject)
        )];
    }
    Vec::new()
}

/// One record produced by an adapter.
///
/// A single enum rather than a trait object per type: the ingestion service
/// needs to validate, meter and publish each kind slightly differently, and an
/// exhaustive match makes adding a record type a compile error everywhere it
/// has to be handled.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case")]
pub enum SensedRecord {
    Tick(Tick),
    Quote(Quote),
    Trade(Trade),
    Book(Box<OrderBook>),
    Bar(Box<Bar>),
    CorporateAction(Box<CorporateAction>),
    News(Box<NewsItem>),
    Fundamental(Box<FundamentalUpdate>),
    Macro(Box<MacroObservation>),
    AlternativeData(Box<AlternativeDataPoint>),
    ReferenceData(Box<ReferenceDataUpdate>),
}

impl SensedRecord {
    /// The topic this record publishes on.
    pub fn topic(&self) -> Topic {
        match self {
            Self::Tick(_) => Topic::MarketTick,
            Self::Quote(_) => Topic::MarketQuote,
            Self::Trade(_) => Topic::MarketTrade,
            Self::Book(_) => Topic::MarketOrderBook,
            Self::Bar(_) => Topic::MarketBar,
            Self::CorporateAction(_) => Topic::MarketCorporateAction,
            Self::News(_) => Topic::NewsReceived,
            Self::Fundamental(_) => Topic::FundamentalUpdated,
            Self::Macro(_) => Topic::MacroUpdated,
            Self::AlternativeData(_) => Topic::AlternativeDataReceived,
            Self::ReferenceData(_) => Topic::ReferenceDataUpdated,
        }
    }

    /// When the fact was true in the world.
    pub fn occurred_at(&self) -> Timestamp {
        match self {
            Self::Tick(t) => t.at,
            Self::Quote(q) => q.at,
            Self::Trade(t) => t.at,
            Self::Book(b) => b.at,
            Self::Bar(b) => b.close_time(),
            Self::CorporateAction(a) => a.ex_date,
            Self::News(n) => n.published_at,
            Self::Fundamental(f) => f.period_end,
            Self::Macro(m) => m.reference_date,
            Self::AlternativeData(a) => a.observed_at,
            Self::ReferenceData(r) => r.effective_from,
        }
    }

    /// The identifier this platform files the record under — the key a
    /// research campaign, a feature store or a reference ledger joins on.
    ///
    /// This is the platform's own id for the thing the record is about (an
    /// `ObjectId`, a macro series id, an entity id), and deliberately not the
    /// source's key for the event: a trade id, `EUR/USD@2026-09-04` or a
    /// Kalshi ticker names the vendor's row, and nothing downstream ever
    /// queries by it. A reference ledger keyed on the vendor's keys held
    /// references no campaign could find, because the campaign asks by
    /// `ObjectId` — which is how a revision to a connector's extent came to
    /// flag nothing. `None` for a news item, which is about entities rather
    /// than one subject and carries none.
    pub fn subject_id(&self) -> Option<&str> {
        match self {
            Self::Tick(t) => Some(t.object_id.as_str()),
            Self::Quote(q) => Some(q.object_id.as_str()),
            Self::Trade(t) => Some(t.object_id.as_str()),
            Self::Book(b) => Some(b.object_id.as_str()),
            Self::Bar(b) => Some(b.object_id.as_str()),
            Self::CorporateAction(a) => Some(a.object_id.as_str()),
            Self::News(_) => None,
            Self::Fundamental(f) => Some(&f.entity_id),
            Self::Macro(m) => Some(&m.series_id),
            Self::AlternativeData(a) => Some(&a.subject_id),
            Self::ReferenceData(r) => Some(&r.object_id),
        }
    }

    /// Subject of the record, for logging and metrics.
    pub fn subject(&self) -> String {
        match self {
            Self::Tick(t) => t.object_id.to_string(),
            Self::Quote(q) => q.object_id.to_string(),
            Self::Trade(t) => t.object_id.to_string(),
            Self::Book(b) => b.object_id.to_string(),
            Self::Bar(b) => b.object_id.to_string(),
            Self::CorporateAction(a) => a.object_id.to_string(),
            Self::News(n) => n.item_id.clone(),
            Self::Fundamental(f) => format!("{}:{}", f.entity_id, f.metric),
            Self::Macro(m) => m.series_id.clone(),
            Self::AlternativeData(a) => format!("{}:{}", a.dataset, a.subject_id),
            Self::ReferenceData(r) => format!("{}:{}", r.object_id, r.field),
        }
    }

    /// Structural problems with the record. Empty means publishable.
    pub fn validate(&self) -> Vec<String> {
        match self {
            Self::Quote(q) => q.validate(),
            Self::Book(b) => b
                .validate()
                .err()
                .map(|e| vec![e.to_string()])
                .unwrap_or_default(),
            Self::Bar(b) => {
                if b.is_coherent() {
                    Vec::new()
                } else {
                    vec![format!(
                        "incoherent bar: open {} high {} low {} close {}",
                        b.open, b.high, b.low, b.close
                    )]
                }
            }
            Self::Trade(t) => {
                let mut issues = Vec::new();
                if !t.price.is_positive() {
                    issues.push(format!("non-positive trade price {}", t.price));
                }
                if t.size.is_negative() {
                    issues.push(format!("negative trade size {}", t.size));
                }
                issues
            }
            Self::Tick(t) => {
                if t.price.is_positive() {
                    Vec::new()
                } else {
                    vec![format!("non-positive tick price {}", t.price)]
                }
            }
            Self::News(n) => {
                let mut issues = Vec::new();
                if n.headline.trim().is_empty() {
                    issues.push("empty headline".into());
                }
                if !(-1.0..=1.0).contains(&n.sentiment.polarity) {
                    issues.push(format!(
                        "sentiment polarity {} out of range",
                        n.sentiment.polarity
                    ));
                }
                issues
            }
            Self::Fundamental(f) => {
                if f.metric.trim().is_empty() {
                    vec!["empty metric name".into()]
                } else {
                    Vec::new()
                }
            }
            Self::Macro(m) => {
                let mut issues = subject_key_issues("macro series id", &m.series_id);
                issues.extend(statistic_issues("macro value", &m.series_id, m.value));
                issues
            }
            Self::AlternativeData(a) => {
                statistic_issues("alternative data value", &a.dataset, a.value)
            }
            Self::CorporateAction(_) | Self::ReferenceData(_) => Vec::new(),
        }
    }

    /// Publish directly onto a bus, outside handler dispatch.
    pub fn publish_to(
        &self,
        bus: &mut EventBus,
        context: &qip_core::Context,
        lineage: qip_core::Lineage,
    ) -> Result<()> {
        let at = self.occurred_at();
        match self {
            Self::Tick(t) => bus.publish(context, lineage, at, t.clone()).map(|_| ()),
            Self::Quote(q) => bus.publish(context, lineage, at, q.clone()).map(|_| ()),
            Self::Trade(t) => bus.publish(context, lineage, at, t.clone()).map(|_| ()),
            Self::Book(b) => bus.publish(context, lineage, at, (**b).clone()).map(|_| ()),
            Self::Bar(b) => bus.publish(context, lineage, at, (**b).clone()).map(|_| ()),
            Self::CorporateAction(a) => {
                bus.publish(context, lineage, at, (**a).clone()).map(|_| ())
            }
            Self::News(n) => bus.publish(context, lineage, at, (**n).clone()).map(|_| ()),
            Self::Fundamental(f) => bus.publish(context, lineage, at, (**f).clone()).map(|_| ()),
            Self::Macro(m) => bus.publish(context, lineage, at, (**m).clone()).map(|_| ()),
            Self::AlternativeData(a) => {
                bus.publish(context, lineage, at, (**a).clone()).map(|_| ())
            }
            Self::ReferenceData(r) => bus.publish(context, lineage, at, (**r).clone()).map(|_| ()),
        }
    }
}

/// What an adapter is and what it may be trusted with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceDescriptor {
    /// Stable adapter name, recorded as the provenance source.
    pub name: String,
    /// Human-readable description of the upstream provider.
    pub provider: String,
    pub licensing: LicensingClass,
    /// Topics this adapter can produce.
    pub topics: Vec<Topic>,
    /// Expected delay between an event and the adapter reporting it.
    pub expected_latency: Duration,
    /// What a production deployment must supply, when the adapter is a stand-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub production_requirement: Option<String>,
}

impl SourceDescriptor {
    /// Whether records from this source may drive a real capital decision.
    pub fn is_production_grade(&self) -> bool {
        self.licensing.allows_production_decisions()
    }
}

/// The common adapter contract.
pub trait DataAdapter: std::fmt::Debug {
    fn descriptor(&self) -> SourceDescriptor;

    /// Produce every record available up to and including `until`.
    ///
    /// Pull rather than push: the caller owns the clock, which is what lets the
    /// same adapter drive a live run, a backtest and a replay.
    fn poll(&mut self, until: Timestamp) -> Result<Vec<SensedRecord>>;

    /// Called once before the first poll.
    fn start(&mut self, _at: Timestamp) -> Result<()> {
        Ok(())
    }

    /// Called when ingestion stops, so the adapter can release resources.
    fn stop(&mut self) -> Result<()> {
        Ok(())
    }

    /// For a source that owns time, move to the next instant at which
    /// something becomes knowable and return it.
    ///
    /// `None` is the answer for every source that does not — a venue, a
    /// vendor, the synthetic exchange — and for a tape that is spent. A caller
    /// that gets `None` runs on its own clock; one that gets an instant runs
    /// the next cycle at it, which is what lets a recorded tape drive a
    /// platform through tape time rather than being swallowed in one poll.
    fn advance(&mut self) -> Option<Timestamp> {
        None
    }

    /// Whether this source owns time — whether [`Self::advance`] is the
    /// clock rather than a question with no answer.
    ///
    /// Distinct from `advance` returning `None`, which a spent tape also
    /// does: a loop has to tell "this source runs on the wall clock" from
    /// "this source ran out", because the first keeps cycling and the second
    /// stops, and a tape that fell back to the wall clock when it ran out
    /// would cycle forever on an empty feed while looking busy.
    fn owns_time(&self) -> bool {
        false
    }
}

/// Marker for adapters producing market data.
pub trait MarketDataAdapter: DataAdapter {
    /// Instruments this adapter covers.
    fn instruments(&self) -> Vec<qip_core::ObjectId>;
}

/// Marker for adapters producing textual intelligence.
pub trait NewsAdapter: DataAdapter {}

/// Marker for adapters producing company fundamentals.
pub trait FundamentalsAdapter: DataAdapter {}

/// Marker for adapters producing macroeconomic series.
pub trait MacroAdapter: DataAdapter {
    fn series(&self) -> Vec<String>;
}

/// Marker for adapters producing alternative data.
pub trait AlternativeDataAdapter: DataAdapter {
    fn datasets(&self) -> Vec<String>;
}

/// A quality failure, ready to publish.
pub fn quality_failure(
    record: &SensedRecord,
    source: &str,
    issues: Vec<String>,
    at: Timestamp,
) -> DataQualityFailure {
    DataQualityFailure {
        source: source.to_string(),
        intended_topic: record.topic().name().to_string(),
        subject_id: Some(record.subject()),
        issues,
        detected_at: at,
        rejected: true,
    }
}

/// Compile-time proof that every SENSE topic has a body type behind it.
pub fn sense_topics() -> Vec<Topic> {
    vec![
        Tick::TOPIC,
        Quote::TOPIC,
        Trade::TOPIC,
        OrderBook::TOPIC,
        Bar::TOPIC,
        CorporateAction::TOPIC,
        NewsItem::TOPIC,
        FundamentalUpdate::TOPIC,
        MacroObservation::TOPIC,
        AlternativeDataPoint::TOPIC,
        ReferenceDataUpdate::TOPIC,
        DataQualityFailure::TOPIC,
    ]
}
