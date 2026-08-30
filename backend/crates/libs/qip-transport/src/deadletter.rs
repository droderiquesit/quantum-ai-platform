//! Where a message goes when the retries run out.
//!
//! # The question this exists to answer
//!
//! "Why did that state delta never arrive." A transport that exhausted its
//! retries and returned an error has answered it for the caller that was
//! holding the message — and for nobody else, because the caller may be a
//! background flush whose error nobody reads. A transport that logged a line
//! and moved on has answered it only for whoever still has the logs.
//!
//! A dead letter keeps the whole frame, not a summary of it. That is the
//! difference between "message 47 failed" and being able to re-send message 47
//! once the peer is back, which is the entire point of recording it.
//!
//! # This store is bounded too, and that costs something
//!
//! [`MemoryDeadLetters`] holds a fixed number of letters and evicts its oldest
//! when it fills, counting the evictions. That is uncomfortable — evicting a
//! dead letter is losing the record of a lost message — and the alternative is
//! worse: an unbounded dead-letter store turns a long peer outage into the
//! out-of-memory kill that the bounded outbound queue exists to prevent, and
//! takes every earlier letter with it.
//!
//! [`DeadLetterSink`] is a trait so a deployment can do better than memory. A
//! sink backed by `qip_events::EventLog` gets an append-only, hash-chained,
//! file-backed record that survives the process, which is what production
//! should wire in. This crate does not do that itself: reaching for the event
//! log from here would invert the dependency between a library and the
//! streaming service that owns the log.

use qip_core::Timestamp;
use qip_events::AnyEvent;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Why a message stopped being retried.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeadLetterReason {
    /// Every attempt the policy allowed was spent and all of them failed.
    RetriesExhausted,
    /// The failure was one that repeating the request cannot change — a
    /// malformed response, a body over the limit, a 4xx from the peer.
    PermanentFailure,
    /// The peer answered and refused the message on its merits.
    Rejected,
}

impl DeadLetterReason {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::RetriesExhausted => "retries exhausted",
            Self::PermanentFailure => "permanent failure",
            Self::Rejected => "rejected by the peer",
        }
    }
}

impl std::fmt::Display for DeadLetterReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One message that was not delivered, and everything needed to say why or to
/// send it again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeadLetter {
    /// The idempotency key, so a letter can be matched against a delivery that
    /// may in fact have landed — see [`crate::mesh`] on lost acknowledgements.
    pub key: String,
    /// Which transport gave up.
    pub transport: String,
    /// Which peer it was going to.
    pub peer: String,
    pub reason: DeadLetterReason,
    /// How many sends were made. Bounded by the retry policy, and recorded so
    /// "it tried once and gave up" is distinguishable from "it tried five
    /// times over six seconds".
    pub attempts: u32,
    /// The last error, as text. The typed error is not kept: this record is
    /// meant to be serialised into a log and read by a person.
    pub last_error: String,
    /// The caller's clock at the moment of the refusal.
    pub recorded_at: Timestamp,
    /// The message itself, so it can be re-sent rather than reconstructed.
    pub frame: AnyEvent,
}

impl DeadLetter {
    /// A one-line summary for an operator.
    pub fn summary(&self) -> String {
        format!(
            "{} to {} after {} attempt(s): {} — {}",
            self.key, self.peer, self.attempts, self.reason, self.last_error
        )
    }
}

/// Somewhere dead letters are kept.
pub trait DeadLetterSink: std::fmt::Debug + Send {
    /// Record one. Infallible by design: a sink that could fail to record a
    /// dead letter would need a dead-letter path of its own, and the recursion
    /// has to stop somewhere. A sink that cannot store the letter must count
    /// the fact that it could not.
    fn record(&mut self, letter: DeadLetter);

    /// How many letters are held right now.
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many letters have ever been recorded, including any since evicted.
    fn recorded(&self) -> u64;

    /// How many were dropped because the sink was full. Non-zero means the
    /// record itself is incomplete, which an operator has to know.
    fn evicted(&self) -> u64;
}

/// A bounded in-memory dead-letter store.
#[derive(Debug)]
pub struct MemoryDeadLetters {
    capacity: usize,
    letters: VecDeque<DeadLetter>,
    recorded: u64,
    evicted: u64,
}

impl MemoryDeadLetters {
    /// Holds at most `capacity` letters, keeping the newest.
    ///
    /// The newest rather than the oldest: during an outage the first failures
    /// and the last failures have the same cause, and the newest are the ones
    /// whose messages are still worth re-sending.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            letters: VecDeque::new(),
            recorded: 0,
            evicted: 0,
        }
    }

    /// Everything held, oldest first.
    pub fn letters(&self) -> Vec<&DeadLetter> {
        self.letters.iter().collect()
    }

    /// The letter recorded for `key`, if it is still held.
    pub fn find(&self, key: &str) -> Option<&DeadLetter> {
        self.letters.iter().find(|letter| letter.key == key)
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Default for MemoryDeadLetters {
    fn default() -> Self {
        Self::new(1024)
    }
}

/// A sink several holders can see.
///
/// A publisher owns its sink by value, which is right — one publisher, one
/// record of what it failed to send — but the whole point of a dead letter is
/// that somebody else reads it. Wrapping the store lets an operator endpoint,
/// a redrive job and the publisher hold the same one.
impl DeadLetterSink for std::sync::Arc<std::sync::Mutex<MemoryDeadLetters>> {
    fn record(&mut self, letter: DeadLetter) {
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .record(letter);
    }

    fn len(&self) -> usize {
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    fn recorded(&self) -> u64 {
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recorded()
    }

    fn evicted(&self) -> u64 {
        self.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .evicted()
    }
}

impl DeadLetterSink for MemoryDeadLetters {
    fn record(&mut self, letter: DeadLetter) {
        if self.letters.len() >= self.capacity {
            self.letters.pop_front();
            self.evicted += 1;
        }
        self.letters.push_back(letter);
        self.recorded += 1;
    }

    fn len(&self) -> usize {
        self.letters.len()
    }

    fn recorded(&self) -> u64 {
        self.recorded
    }

    fn evicted(&self) -> u64 {
        self.evicted
    }
}
