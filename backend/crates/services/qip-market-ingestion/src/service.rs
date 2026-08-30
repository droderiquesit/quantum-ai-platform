//! The ingestion service.
//!
//! Polls adapters, validates every record, publishes what passes, and reports
//! what does not. The validation gate is the platform's promise that bad data
//! never silently becomes an investment decision (charter section 21): a record
//! that fails is published as a
//! [`qip_financial::intelligence::DataQualityFailure`] on its own topic and is
//! visible in metrics, rather than being dropped.

use qip_core::error::Result;
use qip_core::{Context, CorrelationId, Duration, Lineage, Timestamp};
use qip_events::EventBus;
use qip_financial::quality::LicensingClass;
use qip_observability::Telemetry;
use qip_observability::metrics::{labels, names};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::adapter::{DataAdapter, SensedRecord, quality_failure};

/// What ingestion has done so far.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct IngestionStats {
    pub polled: u64,
    pub published: u64,
    pub rejected: u64,
    /// Records published per topic.
    pub by_topic: BTreeMap<String, u64>,
    /// Rejections per adapter.
    pub rejections_by_source: BTreeMap<String, u64>,
    /// Largest observed gap between event time and ingestion time.
    pub worst_latency: Duration,
}

impl IngestionStats {
    /// Fraction of polled records that passed validation.
    pub fn acceptance_rate(&self) -> f64 {
        if self.polled == 0 {
            return 1.0;
        }
        self.published as f64 / self.polled as f64
    }
}

/// Drives adapters and publishes onto the bus.
#[derive(Debug)]
pub struct IngestionService {
    adapters: Vec<Box<dyn DataAdapter>>,
    stats: IngestionStats,
    telemetry: Telemetry,
    /// Refuse to admit records whose licensing class bars production decisions.
    enforce_production_licensing: bool,
}

impl IngestionService {
    pub fn new(telemetry: Telemetry) -> Self {
        Self {
            adapters: Vec::new(),
            stats: IngestionStats::default(),
            telemetry,
            enforce_production_licensing: false,
        }
    }

    /// Reject synthetic sources outright. Set for production environments,
    /// where the kernel must refuse to start on simulated data.
    pub fn enforcing_production_licensing(mut self, enforce: bool) -> Self {
        self.enforce_production_licensing = enforce;
        self
    }

    pub fn register(&mut self, adapter: Box<dyn DataAdapter>) {
        self.telemetry.logger.with(
            qip_observability::Severity::Info,
            "registered data adapter",
            [
                ("adapter", adapter.descriptor().name.as_str()),
                (
                    "licensing",
                    format!("{:?}", adapter.descriptor().licensing).as_str(),
                ),
            ],
        );
        self.adapters.push(adapter);
    }

    pub fn adapter_count(&self) -> usize {
        self.adapters.len()
    }

    pub fn stats(&self) -> &IngestionStats {
        &self.stats
    }

    /// Descriptors of every registered adapter, for the system API.
    pub fn sources(&self) -> Vec<crate::adapter::SourceDescriptor> {
        self.adapters.iter().map(|a| a.descriptor()).collect()
    }

    /// Adapters that are not fit to drive real capital.
    pub fn non_production_sources(&self) -> Vec<String> {
        self.adapters
            .iter()
            .map(|a| a.descriptor())
            .filter(|d| !d.is_production_grade())
            .map(|d| {
                format!(
                    "{}: {}",
                    d.name,
                    d.production_requirement.unwrap_or_default()
                )
            })
            .collect()
    }

    /// Poll every adapter up to `until` and publish what passes validation.
    ///
    /// Each record starts its own lineage chain: it is an originating
    /// observation, and everything derived from it downstream will carry the
    /// correlation id minted here.
    pub fn poll_and_publish(
        &mut self,
        context: &Context,
        bus: &mut EventBus,
        until: Timestamp,
    ) -> Result<u64> {
        let mut published = 0u64;

        for adapter in &mut self.adapters {
            let descriptor = adapter.descriptor();
            if self.enforce_production_licensing && !descriptor.is_production_grade() {
                self.telemetry.logger.with(
                    qip_observability::Severity::Error,
                    "refusing to ingest from a non-production source",
                    [("adapter", descriptor.name.as_str())],
                );
                continue;
            }

            let records = adapter.poll(until)?;
            for record in records {
                self.stats.polled += 1;
                let issues = record.validate();
                let now = context.now();

                if !issues.is_empty() {
                    self.stats.rejected += 1;
                    *self
                        .stats
                        .rejections_by_source
                        .entry(descriptor.name.clone())
                        .or_insert(0) += 1;
                    self.telemetry.metrics.count(
                        names::DATA_VALIDATION_FAILURES,
                        labels([("source", descriptor.name.as_str())]),
                    );

                    let failure = quality_failure(&record, &descriptor.name, issues, now);
                    let correlation: CorrelationId = context.ids().generate(now);
                    bus.publish(
                        context,
                        Lineage::root(correlation, "market-ingestion"),
                        now,
                        failure,
                    )?;
                    continue;
                }

                let occurred_at = record.occurred_at();
                let latency = now.since(occurred_at);
                if latency > self.stats.worst_latency {
                    self.stats.worst_latency = latency;
                }
                self.telemetry.metrics.observe_latency_ms(
                    names::EVENT_LAG_MS,
                    labels([("source", descriptor.name.as_str())]),
                    latency.as_millis() as f64,
                );

                let correlation: CorrelationId = context.ids().generate(now);
                let lineage = Lineage::root(correlation, "market-ingestion");
                record.publish_to(bus, context, lineage)?;

                self.stats.published += 1;
                published += 1;
                *self
                    .stats
                    .by_topic
                    .entry(record.topic().name().to_string())
                    .or_insert(0) += 1;
                self.telemetry.metrics.count(
                    names::EVENTS_PUBLISHED,
                    labels([
                        ("topic", record.topic().name()),
                        ("source", descriptor.name.as_str()),
                    ]),
                );
            }
        }

        Ok(published)
    }

    /// Publish a batch of already-collected records, bypassing the adapters.
    ///
    /// Used by the backtester, which drives the clock itself.
    pub fn publish_records(
        &mut self,
        context: &Context,
        bus: &mut EventBus,
        source: &str,
        records: &[SensedRecord],
    ) -> Result<u64> {
        let mut published = 0;
        for record in records {
            self.stats.polled += 1;
            let issues = record.validate();
            if !issues.is_empty() {
                self.stats.rejected += 1;
                *self
                    .stats
                    .rejections_by_source
                    .entry(source.to_string())
                    .or_insert(0) += 1;
                let failure = quality_failure(record, source, issues, context.now());
                let correlation: CorrelationId = context.ids().generate(context.now());
                bus.publish(
                    context,
                    Lineage::root(correlation, "market-ingestion"),
                    context.now(),
                    failure,
                )?;
                continue;
            }
            let correlation: CorrelationId = context.ids().generate(context.now());
            record.publish_to(bus, context, Lineage::root(correlation, "market-ingestion"))?;
            self.stats.published += 1;
            published += 1;
            *self
                .stats
                .by_topic
                .entry(record.topic().name().to_string())
                .or_insert(0) += 1;
        }
        Ok(published)
    }

    /// Start every adapter.
    pub fn start(&mut self, at: Timestamp) -> Result<()> {
        for adapter in &mut self.adapters {
            adapter.start(at)?;
        }
        Ok(())
    }

    /// Stop every adapter.
    pub fn stop(&mut self) -> Result<()> {
        for adapter in &mut self.adapters {
            adapter.stop()?;
        }
        Ok(())
    }

    /// Whether every registered source is production grade.
    pub fn is_production_ready(&self) -> bool {
        self.adapters
            .iter()
            .all(|a| a.descriptor().licensing != LicensingClass::Synthetic)
    }
}
