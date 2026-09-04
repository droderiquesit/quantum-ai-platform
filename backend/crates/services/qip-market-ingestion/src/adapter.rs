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
                if m.value.is_finite() {
                    Vec::new()
                } else {
                    vec![format!("non-finite macro value for {}", m.series_id)]
                }
            }
            Self::AlternativeData(a) => {
                if a.value.is_finite() {
                    Vec::new()
                } else {
                    vec![format!("non-finite value in {}", a.dataset)]
                }
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
