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

use qip_core::error::Result;
use qip_core::{Context, EventId, Lineage, Timestamp};
use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
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
#[derive(Debug)]
pub struct Publisher<'a> {
    queue: &'a mut VecDeque<AnyEvent>,
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

/// Synchronous, deterministic, replayable event bus.
pub struct EventBus {
    registrations: Vec<Registration>,
    queue: VecDeque<AnyEvent>,
    seen: HashSet<String>,
    log: Option<Rc<RefCell<EventLog>>>,
    failures: Vec<DispatchFailure>,
    dispatched: u64,
    duplicates_suppressed: u64,
    /// Ceiling on events processed in one drain, so a handler loop that
    /// publishes in response to its own output cannot hang the platform.
    max_events_per_drain: usize,
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("subscribers", &self.registrations.len())
            .field("queued", &self.queue.len())
            .field("dispatched", &self.dispatched)
            .field("failures", &self.failures.len())
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
            seen: HashSet::new(),
            log: None,
            failures: Vec::new(),
            dispatched: 0,
            duplicates_suppressed: 0,
            max_events_per_drain: 1_000_000,
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
        let event_id: EventId = context.ids().generate(context.now());
        let envelope = Envelope::new(event_id.clone(), occurred_at, context.now(), lineage, body);
        self.queue.push_back(envelope.erase()?);
        Ok(event_id)
    }

    /// Enqueue an already-erased event, as replay does.
    pub fn publish_raw(&mut self, event: AnyEvent) {
        self.queue.push_back(event);
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// Deliver every queued event, including any published while delivering.
    ///
    /// Returns the number of events dispatched.
    pub fn drain(&mut self, context: &Context) -> Result<usize> {
        let mut processed = 0usize;
        while let Some(mut event) = self.queue.pop_front() {
            if processed >= self.max_events_per_drain {
                return Err(qip_core::Error::guard(format!(
                    "event drain exceeded {} events; a handler is likely publishing in a loop",
                    self.max_events_per_drain
                )));
            }

            let key = event.dedup_key();
            if !self.seen.insert(key) {
                self.duplicates_suppressed += 1;
                continue;
            }

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
                    context,
                    emitted: 0,
                };
                let outcome = handler(&event, &mut publisher);
                self.registrations[index].handler = handler;

                if let Err(error) = outcome {
                    self.failures.push(DispatchFailure {
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

    /// Failures recorded during dispatch.
    pub fn failures(&self) -> &[DispatchFailure] {
        &self.failures
    }

    pub fn clear_failures(&mut self) {
        self.failures.clear();
    }

    pub fn duplicates_suppressed(&self) -> u64 {
        self.duplicates_suppressed
    }

    /// Forget the deduplication set. Replay calls this so a replayed log is not
    /// mistaken for a duplicate of the original run.
    pub fn reset_deduplication(&mut self) {
        self.seen.clear();
    }
}
