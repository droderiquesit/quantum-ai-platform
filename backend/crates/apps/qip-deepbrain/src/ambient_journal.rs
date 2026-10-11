//! The ambient mesh's journal: every signal the research loop's sweep routed
//! and every attention event the router answered with, appended to a
//! hash-chained [`EventLog`] (R12-AMBIENT-1005/1026/1140/1141/1144/7025).
//!
//! # Why this exists
//!
//! The research loop has run the ambient mesh since `b4e9741b`: detections
//! become [`AmbientSignal`]s, the [`qip_contracts::ambient::AttentionRouter`]
//! decides what wakes, and NOW surprises wake named specialists. What it
//! left behind was counters and a cycle line. A counter says *how many*
//! specialists woke; it cannot say which signal woke which specialist, on
//! what evidence, at what severity — and an attention decision nobody can
//! attribute after the process ends is the failure product direction puts
//! first. So each sweep's signals and attention events are written here, in
//! routing order, chained by lineage: each attention event is caused by the
//! signal it answers.
//!
//! # Where it goes
//!
//! Its own log, not the platform's. The platform's log carries the kernel's
//! own bodies on the Discover topics, and an [`AmbientRecord`] under one of
//! those topics there would be a record the kernel's replay decodes as its
//! own type. Here the producer [`AMBIENT_ORIGIN`] and the file tell the two
//! apart, as `qip-agency`'s journal does for the agency's records.
//!
//! # What it may not do
//!
//! Record. Nothing here reads the log back into a decision, and a journaled
//! signal is still an advisory: no `PromotionCase` is built in this file.

use crate::research::ResearchRound;
use qip_contracts::ambient::{AmbientSignal, AttentionEvent};
use qip_core::error::{Error, Result};
use qip_core::lineage::Lineage;
use qip_core::{CorrelationId, EventId, Timestamp};
use qip_events::{Envelope, EventBody, EventLog, Topic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The producer every ambient record carries.
pub const AMBIENT_ORIGIN: &str = "deepbrain/ambient-mesh";

/// The variable naming the ambient journal's file.
pub const AMBIENT_JOURNAL_VARIABLE: &str = "QIP_DEEPBRAIN_AMBIENT_JOURNAL";

/// One ambient record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub enum AmbientRecord {
    /// A signal the sweep routed, whatever the router then did with it.
    Signal { cycle: u64, signal: AmbientSignal },
    /// The router's answer to one signal. `woke` names the specialist an
    /// activated NOW-surprise signal woke.
    Attention {
        cycle: u64,
        event: AttentionEvent,
        woke: Option<String>,
    },
}

impl EventBody for AmbientRecord {
    /// A detection is a Discover fact; see the module documentation for why
    /// this is safe only on the ambient journal's own log.
    const TOPIC: Topic = Topic::SignalGenerated;
    const SCHEMA_VERSION: u32 = 1;
}

/// Where the journal is written, as `main` resolved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalDestination {
    /// Held in the process: the default when the platform's own log is.
    InMemory,
    File(PathBuf),
}

impl JournalDestination {
    /// Resolve the destination from the variable and the platform's own
    /// log. A blank value is refused rather than read as unset. Unset, the
    /// journal is as durable as the platform's log: beside it when that is a
    /// file, so a node whose decisions survive a restart does not keep an
    /// attention record that does not.
    pub fn resolve(
        value: Option<String>,
        platform_log: &qip_kernel::EventLogDestination,
    ) -> Result<Self> {
        match value {
            Some(raw) if raw.trim().is_empty() => Err(Error::invalid(format!(
                "{AMBIENT_JOURNAL_VARIABLE} is set but blank; unset it to journal beside the \
                 event log, or name a file"
            ))),
            Some(raw) => Ok(Self::File(PathBuf::from(raw))),
            None => Ok(match platform_log {
                qip_kernel::EventLogDestination::InMemory => Self::InMemory,
                qip_kernel::EventLogDestination::File { path, .. } => {
                    let mut name = path.as_os_str().to_owned();
                    name.push(".ambient.jsonl");
                    Self::File(PathBuf::from(name))
                }
            }),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::InMemory => "in memory, as the event log is".to_string(),
            Self::File(path) => path.display().to_string(),
        }
    }
}

/// The journal.
#[derive(Debug)]
pub struct AmbientJournal {
    log: EventLog,
    written: u64,
}

impl AmbientJournal {
    /// Open the journal, refusing one whose chain does not verify: a node
    /// that appended to an edited journal would extend a record nobody can
    /// trust.
    pub fn open(destination: &JournalDestination) -> Result<Self> {
        let log = match destination {
            JournalDestination::InMemory => EventLog::in_memory(),
            JournalDestination::File(path) => EventLog::open(path)?,
        };
        if let Err(sequence) = log.verify_chain() {
            return Err(Error::invalid(format!(
                "the ambient journal's hash chain breaks at sequence {sequence}; move the file \
                 aside rather than extend it"
            )));
        }
        // Ids continue from the last sequence, not the retained count: a
        // log that evicted records holds fewer than it ever wrote.
        let written = log.events().last().map_or(0, |e| e.sequence);
        Ok(Self { log, written })
    }

    pub fn log(&self) -> &EventLog {
        &self.log
    }

    /// Append one sweep's signals and attention events, each attention
    /// event caused by the signal it answers. Returns how many were written.
    pub fn record(&mut self, cycle: u64, round: &ResearchRound, at: Timestamp) -> Result<usize> {
        let lineage = Lineage::root(
            CorrelationId::from_string(format!("ambient-sweep-{cycle}-{}", self.written)),
            AMBIENT_ORIGIN,
        );
        let mut by_signal: BTreeMap<&str, EventId> = BTreeMap::new();
        let mut count = 0usize;
        for signal in &round.signals {
            let id = self.append(
                &lineage,
                AmbientRecord::Signal {
                    cycle,
                    signal: signal.clone(),
                },
                signal.detected_at(),
                at,
            )?;
            by_signal.insert(signal.id(), id);
            count += 1;
        }
        for event in &round.attention_events {
            let caused = match by_signal.get(event.signal_id.as_str()) {
                Some(parent) => lineage.caused_by(parent, AMBIENT_ORIGIN),
                // A deferred signal routed in an earlier sweep: its signal
                // record is in that sweep's correlation, not this one's.
                None => lineage.clone(),
            };
            self.append(
                &caused,
                AmbientRecord::Attention {
                    cycle,
                    event: event.clone(),
                    woke: round.woken.get(&event.signal_id).map(|s| (*s).to_string()),
                },
                event.at,
                at,
            )?;
            count += 1;
        }
        Ok(count)
    }

    fn append(
        &mut self,
        lineage: &Lineage,
        record: AmbientRecord,
        occurred_at: Timestamp,
        at: Timestamp,
    ) -> Result<EventId> {
        let event_id = EventId::from_string(format!("evt-ambient-{}", self.written));
        let event =
            Envelope::new(event_id.clone(), occurred_at, at, lineage.clone(), record).erase()?;
        self.log.append(&event)?;
        self.written += 1;
        Ok(event_id)
    }
}

/// Every ambient record in `log`, in log order. One that does not decode is
/// refused, not skipped.
pub fn records(log: &EventLog) -> Result<Vec<AmbientRecord>> {
    let mut out = Vec::new();
    for event in log.events() {
        if event.topic != Topic::SignalGenerated || event.lineage.producer != AMBIENT_ORIGIN {
            continue;
        }
        out.push(event.decode::<AmbientRecord>()?.body);
    }
    Ok(out)
}
