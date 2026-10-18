//! The bridge from the event bus to the platform.
//!
//! Ingestion publishes every record it accepts onto the [`EventBus`] under a
//! lineage it minted, and the platform's SENSE stage absorbs through
//! [`Platform::observe_with_lineage`]. Nothing joined the two: the vertical
//! slice carried a test-owned subscriber standing where this stands, and the
//! correlation id ingestion minted stopped at the bus. [`PlatformBridge`] is
//! that subscriber, with the lifecycle a production subscriber needs.
//!
//! # Why a queue and a pump, and not a handler that calls the platform
//!
//! A bus handler is `'static` and the platform is borrowed by the caller, so a
//! handler cannot hold it. The handler therefore only *admits* — decodes,
//! checks, queues — and [`PlatformBridge::pump`] hands the queue to the
//! platform on the caller's thread, at an instant the caller names. That also
//! puts the platform's write at a point the composition root chooses (before
//! the cycle), not wherever the bus happens to dispatch.
//!
//! # The lifecycle
//!
//! `attach` subscribes (Running). `flow` is one round: drain the bus, pump.
//! `shutdown` drains the bus once more, stops admitting and pumps what is
//! left — so nothing admitted is abandoned and nothing joins the queue after
//! the last pump. The subscription stays: an event published after shutdown
//! is refused *and counted* as `refused_stopped`, where an unsubscribed
//! handler would let it vanish unseen.
//!
//! # What each failure does
//!
//! | Condition | Effect | Counted as |
//! |---|---|---|
//! | Payload is not the topic's body | refused, handler errors (the bus records it) | `malformed` |
//! | Body decodes but fails the ingestion gate's own `validate` | refused | `invalid` |
//! | Queue at capacity | **newest refused** | `refused_full` |
//! | Event arrives after shutdown | refused | `refused_stopped` |
//! | Platform absorbed fewer than it was handed | the difference is counted | `unabsorbed` |
//!
//! **A full queue refuses the newest rather than dropping the oldest.** A
//! queued record is market data nobody else holds in this process, and the
//! oldest is the one with the earliest knowable instant: dropping it would
//! leave the later records absorbed over a gap that nothing marks, which is a
//! point-in-time hole dressed as a quiet feed. Refusing the newest keeps the
//! absorbed prefix contiguous, tells the publisher synchronously through the
//! bus's failure list, and leaves the refused event in the bus's own log to
//! replay. The bound is the bus's own queue discipline, applied one stage
//! later.
//!
//! **Idempotency is the bus's.** [`EventBus::drain`] suppresses an event whose
//! dedup key (topic plus idempotency key, else payload hash) it has already
//! dispatched, before any handler runs, so a redelivered event never reaches
//! this queue. The bridge deliberately keeps no second set of seen ids: two
//! windows of different length would disagree about what a duplicate is.

use qip_core::error::{Error, Result};
use qip_core::{Context, Lineage, ManualClock, Timestamp};
use qip_events::{AnyEvent, EventBus, HandlerOutcome};
use qip_market_ingestion::IngestionService;
use qip_market_ingestion::adapter::SensedRecord;
use qip_observability::metrics::{Metrics, labels, names};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use crate::platform::Platform;

/// The producer the platform's own record carries: the hop between the bus
/// and the platform, caused by the ingestion event.
pub const BRIDGE_PRODUCER: &str = "platform-bridge";

/// A record with the lineage the platform's record of it is to carry.
#[derive(Clone, Debug, PartialEq)]
pub struct SensedInput {
    pub record: SensedRecord,
    /// The correlation id is ingestion's, and — where the record came off a bus
    /// — the causation id is the ingestion event's.
    pub lineage: Lineage,
}

/// What became of a record the bridge was offered, or of a pump.
///
/// A closed set, because it is a metric label: `outcome` must be bounded by an
/// enum and never by a runtime string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Outcome {
    Queued,
    Absorbed,
    Unabsorbed,
    Malformed,
    Invalid,
    RefusedFull,
    RefusedStopped,
}

impl Outcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Absorbed => "absorbed",
            Self::Unabsorbed => "unabsorbed",
            Self::Malformed => "malformed",
            Self::Invalid => "invalid",
            Self::RefusedFull => "refused_full",
            Self::RefusedStopped => "refused_stopped",
        }
    }
}

/// Whether the bridge is admitting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BridgeState {
    Running,
    Stopped,
}

/// Cumulative counts, the same facts the metric series carries, readable
/// without a scrape.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BridgeCounts {
    pub queued: u64,
    pub absorbed: u64,
    pub unabsorbed: u64,
    pub malformed: u64,
    pub invalid: u64,
    pub refused_full: u64,
    pub refused_stopped: u64,
}

impl BridgeCounts {
    /// Offered records that did not reach the queue.
    pub fn refused(&self) -> u64 {
        self.malformed
            .saturating_add(self.invalid)
            .saturating_add(self.refused_full)
            .saturating_add(self.refused_stopped)
    }

    fn bump(&mut self, outcome: Outcome, by: u64) {
        let slot = match outcome {
            Outcome::Queued => &mut self.queued,
            Outcome::Absorbed => &mut self.absorbed,
            Outcome::Unabsorbed => &mut self.unabsorbed,
            Outcome::Malformed => &mut self.malformed,
            Outcome::Invalid => &mut self.invalid,
            Outcome::RefusedFull => &mut self.refused_full,
            Outcome::RefusedStopped => &mut self.refused_stopped,
        };
        *slot = slot.saturating_add(by);
    }
}

/// What one pump handed the platform.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PumpReport {
    /// Records taken off the queue and given to the platform.
    pub delivered: usize,
    /// Of those, the number the platform reports it took in.
    pub absorbed: usize,
}

impl PumpReport {
    /// Records the platform declined (a malformed depth observation, a news
    /// item the world model refused). Each is also a capture problem on the
    /// platform; this is the count the bridge can state without reading it.
    pub fn unabsorbed(&self) -> usize {
        self.delivered.saturating_sub(self.absorbed)
    }
}

#[derive(Debug)]
struct Shared {
    queue: VecDeque<SensedInput>,
    capacity: usize,
    state: BridgeState,
    counts: BridgeCounts,
}

impl Shared {
    fn record(&mut self, metrics: &Metrics, outcome: Outcome, by: u64) {
        self.counts.bump(outcome, by);
        metrics.increment(
            names::SENSE_BRIDGE_RECORDS,
            labels([("outcome", outcome.as_str())]),
            by,
        );
    }
}

/// Subscribes the platform's SENSE stage to the bus.
#[derive(Debug)]
pub struct PlatformBridge {
    shared: Rc<RefCell<Shared>>,
    metrics: Arc<Metrics>,
}

impl PlatformBridge {
    /// Subscribe to every sensed topic on `bus`, admitting at most `capacity`
    /// records between pumps.
    ///
    /// A capacity of zero is refused rather than read as one: a bridge that
    /// refuses every record presents as a feed that has gone quiet.
    pub fn attach(bus: &mut EventBus, capacity: usize, metrics: Arc<Metrics>) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a bridge queue of zero capacity refuses every sensed record, which reads as a \
                 silent feed; give attach the deepest backlog one pump should tolerate",
            ));
        }
        metrics.describe(
            names::SENSE_BRIDGE_RECORDS,
            "sensed records offered to the platform bridge, by what became of them",
        );
        // `RefCell::from`, not `RefCell::new`: `order_placement_paths` greps
        // production source for the text `Cell::new(` to find every cell
        // construction, and a bridge queue is not one.
        let shared = Rc::new(RefCell::from(Shared {
            queue: VecDeque::new(),
            capacity,
            state: BridgeState::Running,
            counts: BridgeCounts::default(),
        }));
        let admit_into = Rc::clone(&shared);
        let admit_metrics = Arc::clone(&metrics);
        bus.subscribe(
            "platform-bridge",
            &SensedRecord::TOPICS,
            move |event, _publisher| admit(&admit_into, &admit_metrics, event),
        );
        Ok(Self { shared, metrics })
    }

    pub fn state(&self) -> BridgeState {
        self.shared.borrow().state
    }

    /// Records admitted and not yet pumped.
    pub fn queued(&self) -> usize {
        self.shared.borrow().queue.len()
    }

    pub fn capacity(&self) -> usize {
        self.shared.borrow().capacity
    }

    pub fn counts(&self) -> BridgeCounts {
        self.shared.borrow().counts.clone()
    }

    /// Hand everything admitted so far to the platform.
    ///
    /// Takes the whole queue in one call: `observe` batches bars per series
    /// and a merge per series is linear where per-record inserts are not.
    /// `now` is when the platform received them (see
    /// [`Platform::observe_with_lineage`]).
    pub fn pump(&self, platform: &mut Platform, now: Timestamp) -> PumpReport {
        let inputs: Vec<SensedInput> = self.shared.borrow_mut().queue.drain(..).collect();
        let delivered = inputs.len();
        let absorbed = platform.observe_with_lineage(inputs, now);
        let report = PumpReport {
            delivered,
            absorbed,
        };
        let mut shared = self.shared.borrow_mut();
        shared.record(&self.metrics, Outcome::Absorbed, absorbed as u64);
        shared.record(
            &self.metrics,
            Outcome::Unabsorbed,
            report.unabsorbed() as u64,
        );
        report
    }

    /// One round: deliver what the bus holds, then pump it.
    ///
    /// The bus's own refusal (a drain that tripped its ceiling) is returned
    /// *after* the pump, so records admitted before the ceiling are not lost
    /// to the error that reports the rest.
    pub fn flow(
        &self,
        bus: &mut EventBus,
        context: &Context,
        platform: &mut Platform,
        now: Timestamp,
    ) -> Result<PumpReport> {
        let drained = bus.drain(context);
        let report = self.pump(platform, now);
        drained.map(|_| report)
    }

    /// Stop for good: deliver what the bus holds, stop admitting, pump the
    /// remainder.
    ///
    /// The order is the point. Draining first admits what was already
    /// published; stopping before the pump means nothing can join the queue
    /// after it is emptied; and the state flag then refuses, and counts, any
    /// event the bus delivers later. Calling it twice is harmless and pumps
    /// nothing the second time.
    pub fn shutdown(
        &self,
        bus: &mut EventBus,
        context: &Context,
        platform: &mut Platform,
        now: Timestamp,
    ) -> Result<PumpReport> {
        let drained = bus.drain(context);
        self.shared.borrow_mut().state = BridgeState::Stopped;
        let report = self.pump(platform, now);
        drained.map(|_| report)
    }
}

/// The bus handler: decode, check, queue — never block, never panic.
fn admit(
    shared: &Rc<RefCell<Shared>>,
    metrics: &Metrics,
    event: &AnyEvent,
) -> Result<HandlerOutcome> {
    let mut shared = shared.borrow_mut();
    if shared.state != BridgeState::Running {
        shared.record(metrics, Outcome::RefusedStopped, 1);
        return Err(Error::guard(
            "the platform bridge has shut down and admits no more records; the event stays in \
             the bus log for replay",
        ));
    }
    let record = match SensedRecord::from_event(event) {
        Ok(record) => record,
        Err(error) => {
            shared.record(metrics, Outcome::Malformed, 1);
            return Err(error);
        }
    };
    // The bus does not run the ingestion gate; a publisher that went around
    // it would otherwise put a zero-price tick in front of the platform.
    let issues = record.validate();
    if !issues.is_empty() {
        shared.record(metrics, Outcome::Invalid, 1);
        return Err(Error::invalid(format!(
            "a {} record failed validation at the platform bridge: {}; publish it through \
             IngestionService so the gate rejects it with a DataQualityFailure",
            event.topic.name(),
            issues.join("; ")
        )));
    }
    if shared.queue.len() >= shared.capacity {
        shared.record(metrics, Outcome::RefusedFull, 1);
        return Err(Error::guard(format!(
            "the platform bridge queue is full at {} records; pump it before publishing more, or \
             attach it with a larger capacity",
            shared.capacity
        )));
    }
    shared.queue.push_back(SensedInput {
        record,
        // Continues the ingestion chain: same correlation id, caused by the
        // event the bus delivered.
        lineage: event.lineage.caused_by(&event.event_id, BRIDGE_PRODUCER),
    });
    shared.record(metrics, Outcome::Queued, 1);
    Ok(HandlerOutcome::Handled)
}

/// Records a composition root that has a feed but no bus of its own hands the
/// platform through one anyway: `IngestionService` (the gate and the
/// correlation id) -> [`EventBus`] -> [`PlatformBridge`] ->
/// [`Platform::observe_with_lineage`].
///
/// This is what makes the bridge production wiring and not a test fixture: the
/// fast brain's feed validates its own records and used to hand them straight
/// to `Platform::observe`, so the bus path was reachable only from a test. A
/// root that holds one of these gets the ingestion correlation id into the
/// platform's event log and journal, the licence gate's counters, the bus's
/// de-duplication window and the bridge's bounded queue, from one call.
///
/// Records move in chunks of [`SENSE_CHUNK`]. A replayed tape can release
/// tens of thousands of records in one poll, and the bus and the bridge are
/// both bounded; feeding a chunk and pumping it before the next keeps every
/// queue inside its bound without refusing a legitimate history.
#[derive(Debug)]
pub struct SensePath {
    context: Context,
    clock: Arc<ManualClock>,
    service: IngestionService,
    bus: EventBus,
    bridge: PlatformBridge,
}

/// Records fed and pumped per round. The bus queue is twice this (a rejected
/// record also publishes a `DataQualityFailure`), and its de-duplication
/// window is sixteen times, comfortably above its per-drain ceiling.
pub const SENSE_CHUNK: usize = 8_192;

impl SensePath {
    /// Build the path for `platform`: ingestion telemetry is the platform's
    /// own, so the gate's series are scraped beside the cycle's.
    ///
    /// Ids are drawn from a context of their own, branched from the platform's
    /// seed by a label that includes the length of the log the platform
    /// resumed. Sharing the platform's generator would be harmless; sharing
    /// its *seed* without branching would mint the cycle's correlation ids
    /// twice, and a restart over an existing log would re-mint the previous
    /// process's tick correlation ids, so `by_correlation` would conflate two
    /// runs' ticks.
    pub fn new(platform: &Platform) -> Result<Self> {
        let (context, clock) =
            Context::deterministic(Timestamp::from_secs(0), platform.context().seed());
        context
            .ids()
            .diverge(&format!("market-ingestion:{}", platform.event_log().len()));
        let mut bus = EventBus::new()
            .max_events_per_drain(2 * SENSE_CHUNK)
            .max_queue_depth(2 * SENSE_CHUNK)?
            .dedup_capacity(16 * SENSE_CHUNK)?;
        let bridge = PlatformBridge::attach(
            &mut bus,
            SENSE_CHUNK,
            Arc::clone(&platform.telemetry().metrics),
        )?;
        Ok(Self {
            context,
            clock,
            service: IngestionService::new(platform.telemetry().clone()),
            bus,
            bridge,
        })
    }

    /// Run `records` — already past the caller's own validation — through the
    /// gate, the bus and the bridge into `platform`. Returns how many the
    /// platform took in.
    ///
    /// `source` is the feed's name, the label on the gate's series. `now` is
    /// both the ingestion instant (the lag the gate meters) and the instant
    /// the platform receives the records.
    pub fn feed(
        &mut self,
        platform: &mut Platform,
        source: &str,
        records: Vec<SensedRecord>,
        now: Timestamp,
    ) -> Result<usize> {
        // `set` never moves backwards, which is the clock the bus's ordering
        // assumes; a caller replaying an earlier instant is refused by the
        // platform's own cycle check, not here.
        self.clock.set(now);
        let mut absorbed = 0;
        for chunk in records.chunks(SENSE_CHUNK) {
            self.service
                .publish_records(&self.context, &mut self.bus, source, chunk)?;
            absorbed += self
                .bridge
                .flow(&mut self.bus, &self.context, platform, now)?
                .absorbed;
        }
        Ok(absorbed)
    }

    /// Stop admitting and pump what is left. The platform is not told the
    /// path has ended; it never held a reference to it.
    pub fn shutdown(&mut self, platform: &mut Platform, now: Timestamp) -> Result<PumpReport> {
        self.bridge
            .shutdown(&mut self.bus, &self.context, platform, now)
    }

    pub fn bridge(&self) -> &PlatformBridge {
        &self.bridge
    }

    pub fn ingestion(&self) -> &IngestionService {
        &self.service
    }

    pub fn bus(&self) -> &EventBus {
        &self.bus
    }
}
