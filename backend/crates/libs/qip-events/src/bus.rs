//! The event bus.
//!
//! Deliberately synchronous and breadth-first. A handler that publishes new
//! events appends them to the queue rather than recursing, so dispatch order
//! depends only on publication order and subscription order — never on thread
//! scheduling. That is what makes a replay reproduce a live run exactly, and it
//! is worth more here than the throughput a work-stealing pool would buy.
//!
//! Handlers are fallible. A failing handler does not abort the drain: the
//! failure is recorded and delivery continues, because one broken consumer must
//! not stop the risk engine from seeing a fill.
//!
//! # Every collection here is bounded, and each overflows differently
//!
//! Every service publishes through this bus, so an unbounded collection in it
//! is an unbounded collection in every process the platform runs. The three
//! that used to grow without limit — the queue, the deduplication set and the
//! failure list — now have explicit capacities set at construction, and each
//! overflows in the direction its contents deserve.
//!
//! * **The queue refuses.** A queued event is a fact nobody else holds — a
//!   fill, a decision, a risk breach. Dropping the oldest would lose it with no
//!   record that it existed, so [`EventBus::publish`] refuses the newest and
//!   the caller finds out synchronously, at the call site, while it still holds
//!   the thing it wanted to publish. [`EventBus::publishes_refused`] counts how
//!   often that happened, which is the number that says whether the queue is
//!   too small or the drain too rare.
//! * **The deduplication set evicts, and says so.** It cannot refuse: refusing
//!   at the set's capacity would stall dispatch of an event that has already
//!   been accepted. So it keeps the most recent
//!   [`EventBus::dedup_window_capacity`] keys and evicts the oldest. Eviction is a
//!   correctness matter, not merely memory — a key forgotten is a duplicate
//!   that can be dispatched twice — so it is counted in
//!   [`EventBus::dedup_evicted`] and the default capacity is at least
//!   `max_events_per_drain`, which makes eviction impossible *within* a single
//!   drain. A non-zero count means the window is too short for the traffic and
//!   the next line of defence is the log's own idempotency key.
//! * **The failure list evicts.** A failure has already happened by the time it
//!   is recorded, so there is nothing to refuse; the only choice is which
//!   failures to keep. It keeps the most recent, because those describe the
//!   current state, and counts what it dropped in
//!   [`EventBus::failures_dropped`] so a handler failing on every event does
//!   not quietly turn into unbounded growth.
//!
//! Under capacity none of this is observable: publication, dispatch order,
//! deduplication and failure recording behave exactly as they did before.

use qip_core::error::{Error, Result};
use qip_core::{Context, EventId, Lineage, Timestamp};
use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;

use crate::envelope::{AnyEvent, Envelope, EventBody};
use crate::log::EventLog;
use crate::topic::Topic;

/// What a handler did with an event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandlerOutcome {
    /// Processed normally.
    Handled,
    /// Deliberately not applicable to this handler.
    Ignored,
}

type Handler = Box<dyn FnMut(&AnyEvent, &mut Publisher<'_>) -> Result<HandlerOutcome>>;

/// A registered handler.
struct Registration {
    name: String,
    topics: Vec<Topic>,
    handler: Handler,
}

impl std::fmt::Debug for Registration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Registration")
            .field("name", &self.name)
            .field("topics", &self.topics)
            .finish_non_exhaustive()
    }
}

/// Handle returned when subscribing, for later removal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subscription {
    pub name: String,
    index: usize,
}

/// A failure recorded during dispatch.
#[derive(Clone, Debug, PartialEq)]
pub struct DispatchFailure {
    pub handler: String,
    pub topic: Topic,
    pub event_id: EventId,
    pub error: String,
}

/// Lets a handler publish without borrowing the whole bus.
///
/// It shares the bus's queue and therefore the bus's capacity: a handler that
/// publishes in response to what it is handling is the most likely way to fill
/// the queue, so it is the last place that may be allowed to bypass the bound.
#[derive(Debug)]
pub struct Publisher<'a> {
    queue: &'a mut VecDeque<AnyEvent>,
    capacity: usize,
    refused: &'a mut u64,
    context: &'a Context,
    emitted: usize,
}

impl<'a> Publisher<'a> {
    /// Publish a typed event caused by the one being handled.
    pub fn publish<T: EventBody>(
        &mut self,
        parent: &AnyEvent,
        producer: &str,
        occurred_at: Timestamp,
        body: T,
    ) -> Result<EventId> {
        let lineage = parent.lineage.caused_by(&parent.event_id, producer);
        self.publish_with_lineage(lineage, occurred_at, body)
    }

    /// Publish with an explicit lineage — used to start a new chain.
    pub fn publish_with_lineage<T: EventBody>(
        &mut self,
        lineage: Lineage,
        occurred_at: Timestamp,
        body: T,
    ) -> Result<EventId> {
        // Refuse before an id is minted. An id generated for an event that was
        // never queued is a gap in the id sequence that a replay cannot explain.
        if self.queue.len() >= self.capacity {
            *self.refused = self.refused.saturating_add(1);
            return Err(queue_full(self.capacity));
        }
        let event_id: EventId = self.context.ids().generate(self.context.now());
        let envelope = Envelope::new(
            event_id.clone(),
            occurred_at,
            self.context.now(),
            lineage,
            body,
        );
        self.queue.push_back(envelope.erase()?);
        self.emitted += 1;
        Ok(event_id)
    }

    /// How many events this handler has published so far.
    pub fn emitted(&self) -> usize {
        self.emitted
    }
}

/// The refusal a full queue returns, in one place so the publisher and the bus
/// cannot drift into saying different things about the same condition.
fn queue_full(capacity: usize) -> Error {
    Error::guard(format!(
        "event queue is full at {capacity} events; drain the bus before publishing more, or \
         construct it with a larger max_queue_depth"
    ))
}

/// Default ceiling on events processed in one drain.
pub const DEFAULT_MAX_EVENTS_PER_DRAIN: usize = 1_000_000;

/// Default ceiling on queue depth. Equal to the drain ceiling, so any queue
/// this bus will accept is a queue one drain could in principle empty; a
/// smaller queue would refuse work the drain was willing to do.
pub const DEFAULT_MAX_QUEUE_DEPTH: usize = DEFAULT_MAX_EVENTS_PER_DRAIN;

/// Default number of deduplication keys remembered. At least the drain ceiling,
/// so no key can be evicted during the drain that inserted it and within-drain
/// deduplication is exact.
pub const DEFAULT_DEDUP_CAPACITY: usize = DEFAULT_MAX_EVENTS_PER_DRAIN;

/// Default number of dispatch failures retained. Failures are diagnostics, not
/// the record — the record is the log — so this is sized to describe an
/// incident rather than to survive one.
pub const DEFAULT_MAX_RECORDED_FAILURES: usize = 10_000;

/// Synchronous, deterministic, replayable event bus.
pub struct EventBus {
    registrations: Vec<Registration>,
    queue: VecDeque<AnyEvent>,
    /// Membership of the deduplication window. A `BTreeSet` rather than a
    /// `HashSet` so that nothing about this bus depends on hash iteration
    /// order, which a replay must never do.
    seen: BTreeSet<String>,
    /// Insertion order for the same window, so the oldest key is the one
    /// evicted. Holding the key twice costs memory the bound now caps.
    seen_order: VecDeque<String>,
    log: Option<Rc<RefCell<EventLog>>>,
    failures: VecDeque<DispatchFailure>,
    dispatched: u64,
    duplicates_suppressed: u64,
    /// Ceiling on events processed in one drain, so a handler loop that
    /// publishes in response to its own output cannot hang the platform.
    max_events_per_drain: usize,
    /// Ceiling on queued events. Reached, publication is refused.
    max_queue_depth: usize,
    /// Ceiling on remembered deduplication keys. Reached, the oldest is
    /// forgotten and the loss is counted.
    dedup_capacity: usize,
    /// Ceiling on retained dispatch failures. Reached, the oldest is dropped
    /// and the loss is counted.
    max_recorded_failures: usize,
    publishes_refused: u64,
    dedup_evicted: u64,
    failures_dropped: u64,
    events_abandoned: u64,
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("subscribers", &self.registrations.len())
            .field("queued", &self.queue.len())
            .field("dispatched", &self.dispatched)
            .field("failures", &self.failures.len())
            .field("refused", &self.publishes_refused)
            .field("dedup_evicted", &self.dedup_evicted)
            .field("failures_dropped", &self.failures_dropped)
            .field("abandoned", &self.events_abandoned)
            .finish()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            registrations: Vec::new(),
            queue: VecDeque::new(),
            seen: BTreeSet::new(),
            seen_order: VecDeque::new(),
            log: None,
            failures: VecDeque::new(),
            dispatched: 0,
            duplicates_suppressed: 0,
            max_events_per_drain: DEFAULT_MAX_EVENTS_PER_DRAIN,
            max_queue_depth: DEFAULT_MAX_QUEUE_DEPTH,
            dedup_capacity: DEFAULT_DEDUP_CAPACITY,
            max_recorded_failures: DEFAULT_MAX_RECORDED_FAILURES,
            publishes_refused: 0,
            dedup_evicted: 0,
            failures_dropped: 0,
            events_abandoned: 0,
        }
    }

    /// Attach a log; every dispatched event is appended before delivery.
    pub fn with_log(mut self, log: Rc<RefCell<EventLog>>) -> Self {
        self.log = Some(log);
        self
    }

    pub fn max_events_per_drain(mut self, limit: usize) -> Self {
        self.max_events_per_drain = limit;
        self
    }

    /// Cap the number of events that may be queued at once.
    ///
    /// Zero is refused rather than read as one. A bus that refuses every
    /// publication presents downstream as a platform that has simply stopped
    /// producing events, which is the hardest kind of misconfiguration to find;
    /// and silently raising it to one would be the same lie in the other
    /// direction, since the operator asked for a bound the code then ignored.
    pub fn max_queue_depth(mut self, capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "an event queue with zero capacity refuses every event the platform produces; \
                 give max_queue_depth the deepest backlog one drain should tolerate",
            ));
        }
        self.max_queue_depth = capacity;
        Ok(self)
    }

    /// Cap the number of deduplication keys remembered.
    ///
    /// Zero is refused: a window that remembers nothing suppresses no duplicate
    /// at all, so every redelivered event would be dispatched again, and the
    /// bus would look like it was deduplicating while doing the opposite.
    pub fn dedup_capacity(mut self, capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a deduplication window of zero remembers nothing, so every redelivered event \
                 would be dispatched again; give dedup_capacity at least max_events_per_drain",
            ));
        }
        self.dedup_capacity = capacity;
        Ok(self)
    }

    /// Cap the number of dispatch failures retained.
    ///
    /// Zero is refused: a bus that records no failure reports a healthy drain
    /// while its handlers are erroring on every event, and
    /// [`EventBus::failures`] is the only place that would have said otherwise.
    pub fn max_recorded_failures(mut self, capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a failure list of zero capacity hides every handler error; give \
                 max_recorded_failures the number of failures an operator should be able to read \
                 back",
            ));
        }
        self.max_recorded_failures = capacity;
        Ok(self)
    }

    /// Subscribe to specific topics.
    pub fn subscribe<F>(
        &mut self,
        name: impl Into<String>,
        topics: &[Topic],
        handler: F,
    ) -> Subscription
    where
        F: FnMut(&AnyEvent, &mut Publisher<'_>) -> Result<HandlerOutcome> + 'static,
    {
        let name = name.into();
        let index = self.registrations.len();
        self.registrations.push(Registration {
            name: name.clone(),
            topics: topics.to_vec(),
            handler: Box::new(handler),
        });
        Subscription { name, index }
    }

    /// Subscribe to one typed event, decoding it for the handler.
    pub fn on<T, F>(&mut self, name: impl Into<String>, mut handler: F) -> Subscription
    where
        T: EventBody,
        F: FnMut(&Envelope<T>, &AnyEvent, &mut Publisher<'_>) -> Result<HandlerOutcome> + 'static,
    {
        self.subscribe(name, &[T::TOPIC], move |any, publisher| {
            let typed = any.decode::<T>()?;
            handler(&typed, any, publisher)
        })
    }

    /// Subscribe to every topic. Used by observability and the audit sink.
    pub fn on_all<F>(&mut self, name: impl Into<String>, handler: F) -> Subscription
    where
        F: FnMut(&AnyEvent, &mut Publisher<'_>) -> Result<HandlerOutcome> + 'static,
    {
        self.subscribe(name, &Topic::ALL, handler)
    }

    pub fn unsubscribe(&mut self, subscription: &Subscription) {
        if subscription.index < self.registrations.len()
            && self.registrations[subscription.index].name == subscription.name
        {
            // Replace with a no-op rather than removing, so other handles keep
            // their indices valid.
            self.registrations[subscription.index].topics.clear();
        }
    }

    pub fn subscriber_count(&self) -> usize {
        self.registrations
            .iter()
            .filter(|r| !r.topics.is_empty())
            .count()
    }

    /// Enqueue a typed event at the root of a new lineage chain.
    pub fn publish<T: EventBody>(
        &mut self,
        context: &Context,
        lineage: Lineage,
        occurred_at: Timestamp,
        body: T,
    ) -> Result<EventId> {
        // Refuse before an id is minted, so the id sequence has no gap that a
        // replay would have to explain.
        if self.queue.len() >= self.max_queue_depth {
            self.publishes_refused = self.publishes_refused.saturating_add(1);
            return Err(queue_full(self.max_queue_depth));
        }
        let event_id: EventId = context.ids().generate(context.now());
        let envelope = Envelope::new(event_id.clone(), occurred_at, context.now(), lineage, body);
        self.queue.push_back(envelope.erase()?);
        Ok(event_id)
    }

    /// Enqueue an already-erased event, as replay does.
    ///
    /// Returns the same refusal as [`EventBus::publish`] when the queue is
    /// full. Replay is exactly the caller that pushes a long history at once,
    /// so it is the caller that most needs to be told it did not all fit
    /// rather than to discover a short replay afterwards.
    ///
    /// Replay runs on a *fresh* bus, whose deduplication window is empty by
    /// construction. There is deliberately no way to forget the window of a
    /// bus that is already running: one existed for replay to call, nothing
    /// called it, and a bus that forgot what it had delivered would, fed its
    /// own log, run every handler's side effects a second time.
    pub fn publish_raw(&mut self, event: AnyEvent) -> Result<()> {
        if self.queue.len() >= self.max_queue_depth {
            self.publishes_refused = self.publishes_refused.saturating_add(1);
            return Err(queue_full(self.max_queue_depth));
        }
        self.queue.push_back(event);
        Ok(())
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// The deepest backlog this bus will hold before refusing.
    pub const fn queue_capacity(&self) -> usize {
        self.max_queue_depth
    }

    /// Publications refused because the queue was full. Non-zero means events
    /// arrive faster than they are drained; nothing was silently dropped, but
    /// somebody was told no and may not have retried.
    pub const fn publishes_refused(&self) -> u64 {
        self.publishes_refused
    }

    /// Deliver every queued event, including any published while delivering.
    ///
    /// Returns the number of events dispatched.
    ///
    /// # What hitting the ceiling leaves behind
    ///
    /// A drain that trips the ceiling abandons whatever is still queued and
    /// says how much. Leaving the backlog in place — which is what this did
    /// once — makes the failure compound: the runaway handler's output is still
    /// there when the next drain starts, so the next drain trips the ceiling
    /// sooner, having dispatched less real work, until nothing gets through at
    /// all. Abandoning is a loss, but a bounded and counted one
    /// ([`EventBus::events_abandoned`]), and every event that was actually
    /// dispatched is already in the log; the queue is not the record.
    pub fn drain(&mut self, context: &Context) -> Result<usize> {
        let mut processed = 0usize;
        while !self.queue.is_empty() {
            if processed >= self.max_events_per_drain {
                let abandoned = self.queue.len();
                self.queue.clear();
                self.events_abandoned = self.events_abandoned.saturating_add(abandoned as u64);
                return Err(Error::guard(format!(
                    "event drain exceeded {} events; a handler is likely publishing in a loop. \
                     {abandoned} queued events were abandoned so the next drain does not start \
                     deeper than this one did; fix the handler, then replay from the log",
                    self.max_events_per_drain
                )));
            }
            let Some(mut event) = self.queue.pop_front() else {
                break;
            };

            let key = event.dedup_key();
            if self.seen.contains(&key) {
                self.duplicates_suppressed += 1;
                continue;
            }
            self.remember(key);

            if let Some(log) = &self.log {
                event.sequence = log.borrow_mut().append(&event)?;
            } else {
                self.dispatched += 1;
                event.sequence = self.dispatched;
            }

            // Take the handler out while it runs so it may publish freely.
            for index in 0..self.registrations.len() {
                if !self.registrations[index].topics.contains(&event.topic) {
                    continue;
                }
                let mut handler = std::mem::replace(
                    &mut self.registrations[index].handler,
                    Box::new(|_, _| Ok(HandlerOutcome::Ignored)),
                );
                let mut publisher = Publisher {
                    queue: &mut self.queue,
                    capacity: self.max_queue_depth,
                    refused: &mut self.publishes_refused,
                    context,
                    emitted: 0,
                };
                let outcome = handler(&event, &mut publisher);
                self.registrations[index].handler = handler;

                if let Err(error) = outcome {
                    self.record_failure(DispatchFailure {
                        handler: self.registrations[index].name.clone(),
                        topic: event.topic,
                        event_id: event.event_id.clone(),
                        error: error.to_string(),
                    });
                }
            }
            processed += 1;
        }
        Ok(processed)
    }

    /// Record a deduplication key, evicting the oldest to stay inside the
    /// window. The eviction is counted because it is a correctness event: past
    /// the window a redelivery is admitted again, and the only remaining
    /// defence is the log's own idempotency key.
    fn remember(&mut self, key: String) {
        while self.seen_order.len() >= self.dedup_capacity {
            match self.seen_order.pop_front() {
                Some(oldest) => {
                    self.seen.remove(&oldest);
                    self.dedup_evicted = self.dedup_evicted.saturating_add(1);
                }
                None => break,
            }
        }
        self.seen.insert(key.clone());
        self.seen_order.push_back(key);
    }

    /// Retain a dispatch failure, dropping the oldest to stay inside the cap.
    fn record_failure(&mut self, failure: DispatchFailure) {
        while self.failures.len() >= self.max_recorded_failures {
            if self.failures.pop_front().is_none() {
                break;
            }
            self.failures_dropped = self.failures_dropped.saturating_add(1);
        }
        self.failures.push_back(failure);
    }

    /// Failures recorded during dispatch, oldest first.
    ///
    /// Bounded by [`EventBus::failure_capacity`]; when more failures occurred
    /// than are retained, [`EventBus::failures_dropped`] is non-zero and this
    /// is the tail, not the whole story.
    pub fn failures(&self) -> impl ExactSizeIterator<Item = &DispatchFailure> {
        self.failures.iter()
    }

    /// How many failures are currently retained.
    pub fn failure_count(&self) -> usize {
        self.failures.len()
    }

    /// The most failures this bus will retain.
    pub const fn failure_capacity(&self) -> usize {
        self.max_recorded_failures
    }

    /// Failures discarded to stay inside the cap. Non-zero means a handler is
    /// failing far more often than anyone has read the failures.
    pub const fn failures_dropped(&self) -> u64 {
        self.failures_dropped
    }

    pub fn duplicates_suppressed(&self) -> u64 {
        self.duplicates_suppressed
    }

    /// The most deduplication keys this bus remembers at once.
    pub const fn dedup_window_capacity(&self) -> usize {
        self.dedup_capacity
    }

    /// Keys forgotten to stay inside the window. Non-zero means a duplicate
    /// older than the window would now be dispatched a second time.
    pub const fn dedup_evicted(&self) -> u64 {
        self.dedup_evicted
    }

    /// Events discarded because a drain hit its ceiling with work still queued.
    pub const fn events_abandoned(&self) -> u64 {
        self.events_abandoned
    }
}
