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
use qip_financial::intelligence::DataQualityFailure;
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
    /// Records that passed the validation gate.
    ///
    /// Distinct from `published`, which counts what reached a bus. A caller
    /// using [`IngestionService::poll_batch`] publishes nothing at all, and
    /// `acceptance_rate` computed from `published` would report a clean feed as
    /// zero per cent accepted — a number that reads as a broken source.
    #[serde(default)]
    pub accepted: u64,
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
        self.accepted as f64 / self.polled as f64
    }
}

/// One poll's worth of sifted records, for a caller that has no event bus.
///
/// The service's other paths publish onto a [`qip_events::EventBus`], and no
/// composition root in this workspace constructs one — so the validation gate
/// that turns a bad record into a [`DataQualityFailure`] was reachable only
/// from this crate's own tests, while the deployed node carried a thinner copy
/// of the same check whose rejections were formatted strings no metric could
/// see. This is the shape a root can take: records it hands straight on,
/// failures it must account for, and the sources the licensing posture refused
/// to poll at all.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SensedBatch {
    /// Records that passed validation, in adapter registration order.
    pub accepted: Vec<SensedRecord>,
    /// One per rejected record, ready to publish, log or journal. A caller that
    /// drops these has re-created the defect this type exists to end.
    pub failures: Vec<DataQualityFailure>,
    /// Adapters not polled because their licensing class bars a production
    /// decision. Returned rather than only logged: a node whose every source
    /// was refused would otherwise report a successful poll of nothing.
    pub refused_sources: Vec<String>,
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

    /// The one validation gate.
    ///
    /// Every path through this service goes through here, so a rejection is
    /// counted, metered and shaped identically whichever entry point the caller
    /// took. Two copies of this check is how `publish_records` came to reject
    /// records that `qip_data_validation_failures_total` never saw, and how the
    /// fast brain came to carry a third copy whose rejections were strings.
    ///
    /// `now` is the ingestion instant and the record carries its own occurrence
    /// instant; the gap between them is the lag recorded here. They are separate
    /// because a backtest's two clocks are not the same clock.
    fn sift(
        &mut self,
        record: &SensedRecord,
        source: &str,
        now: Timestamp,
    ) -> Option<DataQualityFailure> {
        self.stats.polled += 1;
        let issues = record.validate();
        if !issues.is_empty() {
            self.stats.rejected += 1;
            *self
                .stats
                .rejections_by_source
                .entry(source.to_string())
                .or_insert(0) += 1;
            self.telemetry.metrics.count(
                names::DATA_VALIDATION_FAILURES,
                labels([("source", source)]),
            );
            return Some(quality_failure(record, source, issues, now));
        }

        let latency = now.since(record.occurred_at());
        if latency > self.stats.worst_latency {
            self.stats.worst_latency = latency;
        }
        self.telemetry.metrics.observe_latency_ms(
            names::EVENT_LAG_MS,
            labels([("source", source)]),
            latency.as_millis() as f64,
        );
        self.stats.accepted += 1;
        None
    }

    /// Publish one record that has already passed the gate.
    ///
    /// `qip_events_published_total` is recorded here and nowhere else, because
    /// it is a claim about publication and not about acceptance. `accepted` and
    /// `published` are two counts of two different facts, and a caller taking
    /// [`Self::poll_batch`] moves the first without the second.
    fn publish_one(
        &mut self,
        context: &Context,
        bus: &mut EventBus,
        record: &SensedRecord,
        source: &str,
        now: Timestamp,
    ) -> Result<()> {
        let correlation: CorrelationId = context.ids().generate(now);
        record.publish_to(bus, context, Lineage::root(correlation, "market-ingestion"))?;
        self.stats.published += 1;
        *self
            .stats
            .by_topic
            .entry(record.topic().name().to_string())
            .or_insert(0) += 1;
        self.telemetry.metrics.count(
            names::EVENTS_PUBLISHED,
            labels([("topic", record.topic().name()), ("source", source)]),
        );
        Ok(())
    }

    /// Publish a [`DataQualityFailure`] the gate produced.
    fn publish_failure(
        &self,
        context: &Context,
        bus: &mut EventBus,
        failure: DataQualityFailure,
        now: Timestamp,
    ) -> Result<()> {
        let correlation: CorrelationId = context.ids().generate(now);
        bus.publish(
            context,
            Lineage::root(correlation, "market-ingestion"),
            now,
            failure,
        )?;
        Ok(())
    }

    /// Whether this adapter's licensing class bars it from being polled, given
    /// the configured enforcement. Logs and names the source when it does.
    fn refuses(&self, descriptor: &crate::adapter::SourceDescriptor) -> bool {
        if !self.enforce_production_licensing || descriptor.is_production_grade() {
            return false;
        }
        self.telemetry.logger.with(
            qip_observability::Severity::Error,
            "refusing to ingest from a non-production source",
            [("adapter", descriptor.name.as_str())],
        );
        true
    }

    /// Poll every permitted adapter up to `until` and return what the gate made
    /// of the records, without publishing anything.
    ///
    /// This is the call a composition root can make. [`Self::poll_and_publish`]
    /// needs a [`qip_events::EventBus`], which nothing in this workspace
    /// constructs, so until one exists this is the only way the gate — and the
    /// two metric series only this file emits — can reach a deployed process.
    ///
    /// `now` is when the platform learned the records; `until` bounds what the
    /// adapter may report by event time. Keeping them apart is what lets the
    /// same adapter drive a live run and a replay.
    pub fn poll_batch(&mut self, now: Timestamp, until: Timestamp) -> Result<SensedBatch> {
        let mut batch = SensedBatch::default();
        // Indexed rather than iterated: `sift` takes `&mut self`, and a mutable
        // borrow of `self.adapters` cannot be held across it.
        for index in 0..self.adapters.len() {
            let descriptor = self.adapters[index].descriptor();
            if self.refuses(&descriptor) {
                batch.refused_sources.push(descriptor.name.clone());
                continue;
            }

            let records = self.adapters[index].poll(until)?;
            for record in records {
                match self.sift(&record, &descriptor.name, now) {
                    Some(failure) => batch.failures.push(failure),
                    None => batch.accepted.push(record),
                }
            }
        }
        Ok(batch)
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
        let now = context.now();
        let mut published = 0u64;

        // Indexed for the same reason as `poll_batch`: the gate takes
        // `&mut self`. The adapter loop is not shared with `poll_batch` because
        // this path needs the per-record source in scope for the publication
        // metric's label, which `SensedBatch` deliberately does not carry.
        for index in 0..self.adapters.len() {
            let descriptor = self.adapters[index].descriptor();
            if self.refuses(&descriptor) {
                continue;
            }

            let records = self.adapters[index].poll(until)?;
            for record in records {
                match self.sift(&record, &descriptor.name, now) {
                    Some(failure) => self.publish_failure(context, bus, failure, now)?,
                    None => {
                        self.publish_one(context, bus, &record, &descriptor.name, now)?;
                        published += 1;
                    }
                }
            }
        }

        Ok(published)
    }

    /// Publish a batch of already-collected records, bypassing the adapters.
    ///
    /// Used by the backtester, which drives the clock itself. It runs the same
    /// gate as every other path: this arm once carried its own copy that
    /// recorded no metric at all, so a rejection here moved nothing an operator
    /// could see.
    pub fn publish_records(
        &mut self,
        context: &Context,
        bus: &mut EventBus,
        source: &str,
        records: &[SensedRecord],
    ) -> Result<u64> {
        let now = context.now();
        let mut published = 0;
        for record in records {
            match self.sift(record, source, now) {
                Some(failure) => self.publish_failure(context, bus, failure, now)?,
                None => {
                    self.publish_one(context, bus, record, source, now)?;
                    published += 1;
                }
            }
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

#[cfg(test)]
mod tests {
    //! These sit beside the code rather than in `tests/`: they assert this
    //! type's own invariants — that one gate serves every entry point — and
    //! `sift` is private, so nothing outside the module can reach the seam.

    use super::*;
    use qip_core::{Decimal, ObjectId, dec};
    use qip_financial::quality::LicensingClass;
    use qip_market::quote::{Quote, Trade, TradeCondition};

    fn at() -> Timestamp {
        Timestamp::parse_rfc3339("2026-08-24T15:00:00Z").expect("a literal RFC 3339 instant")
    }

    /// An adapter that reports exactly the records the test gives it.
    ///
    /// The synthetic environment cannot stand in here: it only produces valid
    /// records, so a test of the rejecting arm would have nothing to reject.
    #[derive(Debug)]
    struct FixedAdapter {
        name: String,
        licensing: LicensingClass,
        records: Vec<SensedRecord>,
    }

    impl DataAdapter for FixedAdapter {
        fn descriptor(&self) -> crate::adapter::SourceDescriptor {
            crate::adapter::SourceDescriptor {
                name: self.name.clone(),
                provider: format!("{}-provider", self.name),
                licensing: self.licensing,
                topics: vec![qip_events::Topic::MarketTrade],
                expected_latency: Duration::ZERO,
                production_requirement: None,
            }
        }

        fn poll(&mut self, _until: Timestamp) -> Result<Vec<SensedRecord>> {
            Ok(std::mem::take(&mut self.records))
        }
    }

    /// A quote whose bid is above its ask — the record `Quote::validate`
    /// refuses.
    fn crossed_quote() -> SensedRecord {
        SensedRecord::Quote(Quote {
            object_id: ObjectId::from_string("OBJ0000000000000000000001"),
            venue: "XNYS".into(),
            at: at(),
            bid: dec!("100.20"),
            ask: dec!("100.10"),
            bid_size: Decimal::from_int(100),
            ask_size: Decimal::from_int(100),
            quality: Default::default(),
        })
    }

    fn good_trade() -> SensedRecord {
        SensedRecord::Trade(Trade {
            object_id: ObjectId::from_string("OBJ0000000000000000000001"),
            venue: "XNYS".into(),
            at: at(),
            price: dec!("100.15"),
            size: Decimal::from_int(50),
            aggressor: None,
            condition: TradeCondition::Regular,
            trade_id: Some("t1".into()),
            quality: Default::default(),
        })
    }

    #[test]
    fn a_record_that_fails_the_gate_is_returned_as_a_failure_and_counted_even_when_no_bus_exists() {
        let telemetry = Telemetry::silent();
        let mut service = IngestionService::new(telemetry.clone());
        let adapter = FixedAdapter {
            name: "fixed".into(),
            licensing: LicensingClass::Licensed,
            records: vec![crossed_quote(), good_trade()],
        };

        // Premise, both halves. The source is production grade, so the
        // licensing arm is not what produces the result below; and the counter
        // starts at zero, so a `1` afterwards is this call's doing.
        assert!(
            adapter.descriptor().is_production_grade(),
            "the fixture must pass the licensing arm, or this proves that arm instead"
        );
        assert_eq!(
            telemetry
                .metrics
                .snapshot()
                .counter_total(names::DATA_VALIDATION_FAILURES),
            0,
            "the validation counter must start at zero for the assertion below to mean anything"
        );

        service.register(Box::new(adapter));
        let batch = service
            .poll_batch(at(), at())
            .expect("the fixture cannot fail to poll");

        assert_eq!(batch.accepted.len(), 1, "the valid trade must be accepted");
        assert_eq!(
            batch.failures.len(),
            1,
            "the crossed quote must be a failure"
        );
        assert!(
            !batch.failures[0].issues.is_empty(),
            "a failure must say what was wrong with the record"
        );
        // Equality, not `contains`: a source named `fixed` is a substring of a
        // source named `fixed-2`, and this repository has already shipped a
        // test that survived deleting the value it guarded for that reason.
        assert_eq!(batch.failures[0].source, "fixed");
        assert_eq!(
            telemetry
                .metrics
                .snapshot()
                .counter_total(names::DATA_VALIDATION_FAILURES),
            1,
            "the bus-free path must move the same counter the publishing path does"
        );
    }

    #[test]
    fn the_licensing_gate_names_the_source_it_refused_rather_than_polling_it() {
        let synthetic = || FixedAdapter {
            name: "synthetic-fixture".into(),
            licensing: LicensingClass::Synthetic,
            records: vec![good_trade()],
        };

        // Premise: the same adapter unenforced yields records. Without this, an
        // empty `accepted` under enforcement is also what an adapter producing
        // nothing would give, and the gate would be proven by silence.
        let mut unenforced = IngestionService::new(Telemetry::silent());
        unenforced.register(Box::new(synthetic()));
        let permitted = unenforced
            .poll_batch(at(), at())
            .expect("the fixture cannot fail to poll");
        assert_eq!(
            permitted.accepted.len(),
            1,
            "unenforced, the fixture must produce a record, or the refusal below proves nothing"
        );
        assert!(permitted.refused_sources.is_empty());

        let mut service =
            IngestionService::new(Telemetry::silent()).enforcing_production_licensing(true);
        service.register(Box::new(synthetic()));
        let batch = service
            .poll_batch(at(), at())
            .expect("the fixture cannot fail to poll");

        assert!(
            batch.accepted.is_empty(),
            "a synthetic source must not be polled"
        );
        assert!(
            batch.failures.is_empty(),
            "a refused source produces no records, so it produces no quality failures either"
        );
        // Full equality on the whole list: `contains` on a name would also
        // match a longer name that embeds it, and would not notice the
        // descriptor's provider being pushed here instead of its name.
        assert_eq!(batch.refused_sources, vec!["synthetic-fixture".to_string()]);
    }

    #[test]
    fn the_backtester_path_counts_a_published_record_in_the_same_series_as_the_polling_path() {
        let telemetry = Telemetry::silent();
        let (context, _clock) = Context::deterministic(at(), 1);
        let mut bus = EventBus::new();
        let mut service = IngestionService::new(telemetry.clone());

        assert_eq!(
            telemetry
                .metrics
                .snapshot()
                .counter_total(names::EVENTS_PUBLISHED),
            0,
            "the publication counter must start at zero"
        );

        // The backtester's arm. It once recorded nothing at all, so a rejection
        // or a publication here moved no series an operator could read.
        let published = service
            .publish_records(&context, &mut bus, "backtest", &[good_trade()])
            .expect("an in-memory bus cannot fail to accept a valid trade");
        assert_eq!(published, 1);
        assert_eq!(
            telemetry
                .metrics
                .snapshot()
                .counter_total(names::EVENTS_PUBLISHED),
            1,
            "publishing through the backtester arm must move the publication counter"
        );

        service.register(Box::new(FixedAdapter {
            name: "fixed".into(),
            licensing: LicensingClass::Licensed,
            records: vec![good_trade()],
        }));
        let polled = service
            .poll_and_publish(&context, &mut bus, at())
            .expect("an in-memory bus cannot fail to accept a valid trade");
        assert_eq!(polled, 1);
        assert_eq!(
            telemetry
                .metrics
                .snapshot()
                .counter_total(names::EVENTS_PUBLISHED),
            2,
            "both arms must count into one series, or the two claims can never disagree"
        );
    }
}
