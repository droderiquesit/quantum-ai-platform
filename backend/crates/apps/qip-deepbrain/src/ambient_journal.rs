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

use crate::research::{AssumptionState, ResearchRound};
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
    /// A conditional scenario the sweep generated from one signal
    /// (R12-AMBIENT-1005), caused by that signal's record.
    Scenario {
        cycle: u64,
        signal_id: String,
        scenario: qip_simulation_engine::scenario::Scenario,
        /// That the scenario was generated and from what inputs
        /// (R12-EXPAND-6033). Absent on records written before it existed.
        #[serde(default)]
        episode: Option<qip_contracts::expansion::SyntheticEpisode>,
    },
    /// Every assumption the loop holds and how it stands after the sweep
    /// (R12-AMBIENT-1143). Written every sweep the table is non-empty, so
    /// a sweep that challenged nothing still records what is holding.
    AssumptionHealth {
        cycle: u64,
        assumptions: Vec<AssumptionState>,
    },
    /// The plan the sweep's started items compile to (R12-REASON-3061).
    SweepPlan {
        cycle: u64,
        plan: crate::research_plan::SweepPlanRecord,
    },
    /// A tick campaign concluded on its re-measured venue
    /// (R12-EXPAND-6042): the microstructure delta, whose id is the
    /// campaign's.
    MicrostructureDelta {
        cycle: u64,
        delta: qip_contracts::microstructure_delta::MicrostructureDelta,
    },
    /// One source scored on one subject by the source-alpha desk
    /// (R12-EXPAND-6027): the validated contract record, with the window
    /// and the digest of the samples it was computed from.
    SourceScore {
        cycle: u64,
        score: qip_contracts::expansion::SourceScore,
    },
    /// The compute market the sweep ran (R12-AMBIENT-9034): the agent
    /// budgets it allocated within, each grant with the reputation weight
    /// it was priced on, each decline with its reason, and what was spent.
    /// Written for every sweep in which something bid.
    Market {
        cycle: u64,
        market: crate::research::MarketRecord,
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
    /// The correlation the latest [`Self::record`] filed its sweep under, so
    /// the agency intake files a draft under the same chain
    /// (R12-AGENCY-7029): a draft whose correlation is not the detection's
    /// cannot be joined back to it.
    last_correlation: Option<CorrelationId>,
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
        Ok(Self {
            log,
            written,
            last_correlation: None,
        })
    }

    pub fn log(&self) -> &EventLog {
        &self.log
    }

    /// The correlation id of the sweep most recently recorded.
    pub fn last_correlation(&self) -> Option<&CorrelationId> {
        self.last_correlation.as_ref()
    }

    /// Append one sweep's signals and attention events, each attention
    /// event caused by the signal it answers. Returns how many were written.
    /// Write one record per source score the desk produced this cycle
    /// (R12-EXPAND-6027), under its own lineage: a score is the platform's
    /// finding about a source, not a signal the sweep routed.
    pub fn record_source_scores(
        &mut self,
        cycle: u64,
        round: &crate::source_alpha::SourceAlphaRound,
        at: Timestamp,
    ) -> Result<usize> {
        let lineage = Lineage::root(
            CorrelationId::from_string(format!("source-alpha-{cycle}-{}", self.written)),
            AMBIENT_ORIGIN,
        );
        let mut count = 0usize;
        for score in &round.records {
            self.append(
                &lineage,
                AmbientRecord::SourceScore {
                    cycle,
                    score: score.clone(),
                },
                score.scored_at,
                at,
            )?;
            count += 1;
        }
        Ok(count)
    }

    pub fn record(&mut self, cycle: u64, round: &ResearchRound, at: Timestamp) -> Result<usize> {
        let lineage = Lineage::root(
            CorrelationId::from_string(format!("ambient-sweep-{cycle}-{}", self.written)),
            AMBIENT_ORIGIN,
        );
        self.last_correlation = Some(lineage.correlation_id.clone());
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
        for generated in &round.scenarios {
            let caused = match by_signal.get(generated.signal_id.as_str()) {
                Some(parent) => lineage.caused_by(parent, AMBIENT_ORIGIN),
                None => lineage.clone(),
            };
            self.append(
                &caused,
                AmbientRecord::Scenario {
                    cycle,
                    signal_id: generated.signal_id.clone(),
                    scenario: generated.scenario.clone(),
                    episode: Some(generated.episode.clone()),
                },
                at,
                at,
            )?;
            count += 1;
        }
        for delta in &round.deltas {
            self.append(
                &lineage,
                AmbientRecord::MicrostructureDelta {
                    cycle,
                    delta: delta.clone(),
                },
                at,
                at,
            )?;
            count += 1;
        }
        if let Some(market) = &round.market {
            self.append(
                &lineage,
                AmbientRecord::Market {
                    cycle,
                    market: market.clone(),
                },
                at,
                at,
            )?;
            count += 1;
        }
        if let Some(plan) = &round.plan {
            self.append(
                &lineage,
                AmbientRecord::SweepPlan {
                    cycle,
                    plan: plan.clone(),
                },
                at,
                at,
            )?;
            count += 1;
        }
        if !round.assumptions.is_empty() {
            self.append(
                &lineage,
                AmbientRecord::AssumptionHealth {
                    cycle,
                    assumptions: round.assumptions.clone(),
                },
                at,
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
