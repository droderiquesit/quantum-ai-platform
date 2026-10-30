//! What the deep brain does when the AI platform is lost (ADR 0214,
//! R12-RES-10130).
//!
//! # The signal
//!
//! "The AI platform is lost" is not a new measurement. It is two of the
//! states this node's own readiness already reports
//! ([`NodeStatus::self_observed_outage`]): halted by the kill switch, or
//! persistently failing. `/ready` takes the node out of rotation on
//! each, and the forecast lattice's strategic service keys on the same
//! predicate (`node::gate_strategic_service`); it is computed here once so
//! the two cannot disagree. No threshold is added: the failure tolerance is
//! the one the configuration already holds.
//!
//! Three readiness states are not a loss. *Warming*: every node warms, and
//! holding its first rounds would be a backlog nothing went wrong to cause.
//! *Stalled*: the run loop asks this question at the top of a cycle, so it
//! has just come back. A stall seen from inside the loop is a clock that
//! jumped between cycles — a tape whose periods are a day apart, a process
//! that was suspended — and not a platform that stopped thinking; a wedged
//! node cannot ask. The probe, on its own thread, still reports it.
//! *Stopping*: set only after the loop ends, so a loop that reads it is a
//! second run over a finished status; a stop is a request to end, not a
//! loss.
//!
//! # What it holds
//!
//! While the signal holds, every loop in the gap after a cycle that produces
//! *new* cognition — a strategy search and its challenge (which may crown a
//! champion), a learning round (which may fit a model and promote a
//! package), a research sweep with its scouting, journal and agency drafts,
//! and a tick-desk round (labels, models, fused priors) — produces nothing.
//! A round that was due on the cycle is written to the [`ResearchBacklog`]
//! instead: bounded, hash-chained, in the order it was held. When the signal
//! clears, the backlog is drained in that order before the cycle's own
//! rounds run.
//!
//! The cycle itself is not held. SENSE through LEARN run on the packages and
//! models already installed, which is what "existing packages continue"
//! means; nothing here promotes a package, and nothing here withdraws one.
//!
//! # Refusals
//!
//! A full backlog refuses the held work and journals the refusal; it never
//! evicts what it already holds and never lets the loop run because there is
//! no room to queue it. A held round with nowhere to go is a side failure on
//! the cycle line, not a reason to think anyway.

use crate::status::{NodeStatus, Unready};
use qip_core::error::{Error, Result};
use qip_core::lineage::Lineage;
use qip_core::{CorrelationId, EventId, Timestamp};
use qip_events::{Envelope, EventBody, EventLog, Topic};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;

/// The producer every backlog record carries.
pub const BACKLOG_ORIGIN: &str = "deepbrain/research-backlog";

/// The most held rounds the backlog keeps.
///
/// The bound the kernel already puts on the strategic questions the same
/// outage holds (`LATTICE_MAX_BACKLOG`), not a second number: one outage, one
/// bound on what waits for it. At the default five-minute cadence it is more
/// than three days of held cycles.
pub const RESEARCH_BACKLOG_CAPACITY: usize = qip_kernel::lattice_stage::LATTICE_MAX_BACKLOG;

/// Whether the AI platform is lost, and why. See the module documentation
/// for why warming and stalled are not.
pub fn ai_platform_lost(status: &NodeStatus) -> Option<Unready> {
    status.self_observed_outage()
}

/// A loop that produces new cognition, and so is held while the platform is
/// lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CognitiveLoop {
    /// The evolution engine's search round and its champion contest.
    Search,
    /// The learning desk's round: a fit, a drift score, a package promotion.
    Learning,
    /// The research sweep, its scout hand-off, journal and agency drafts.
    Research,
    /// The tick desk's round: replay, labels, models, fusion.
    Tick,
}

impl CognitiveLoop {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Search => "search",
            Self::Learning => "learning",
            Self::Research => "research",
            Self::Tick => "tick",
        }
    }
}

/// One cycle's held rounds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeldWork {
    /// The cycle the rounds were due on. Drained, each loop is asked again
    /// with this number, so a round runs exactly when it was due.
    pub cycle: u64,
    pub held_at: Timestamp,
    /// Why the platform was lost when it was held.
    pub cause: Unready,
    pub loops: BTreeSet<CognitiveLoop>,
}

/// One backlog record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "entry", rename_all = "snake_case")]
pub enum BacklogRecord {
    Held {
        work: HeldWork,
    },
    /// Held work the backlog had no room for. Journaled so a gap in what
    /// was later drained is a recorded refusal and not a silence.
    Refused {
        work: HeldWork,
        capacity: usize,
    },
    /// Held work handed back to its loops, in the order it was held.
    Drained {
        cycle: u64,
        drained_at: Timestamp,
    },
}

impl EventBody for BacklogRecord {
    /// On the backlog's own log only, as the ambient journal's records are on
    /// theirs: the platform's replay never decodes one.
    const TOPIC: Topic = Topic::SystemAlert;
    const SCHEMA_VERSION: u32 = 1;
}

/// Where the backlog is journaled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BacklogDestination {
    InMemory,
    File(PathBuf),
}

impl BacklogDestination {
    /// Beside the platform's own log when that is a file, in memory when it
    /// is: a node whose decisions survive a restart keeps a backlog that
    /// does too, and one whose decisions do not is not pretending otherwise.
    pub fn beside(platform_log: &qip_kernel::EventLogDestination) -> Self {
        match platform_log {
            qip_kernel::EventLogDestination::InMemory => Self::InMemory,
            qip_kernel::EventLogDestination::File { path, .. } => {
                let mut name = path.as_os_str().to_owned();
                name.push(".backlog.jsonl");
                Self::File(PathBuf::from(name))
            }
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Self::InMemory => "in memory, as the event log is".to_string(),
            Self::File(path) => path.display().to_string(),
        }
    }
}

/// The research backlog: what the lost platform did not think about, in the
/// order it did not think about it.
#[derive(Debug)]
pub struct ResearchBacklog {
    log: EventLog,
    pending: VecDeque<HeldWork>,
    capacity: usize,
    written: u64,
    refused: u64,
}

impl ResearchBacklog {
    /// Open the backlog at [`RESEARCH_BACKLOG_CAPACITY`], restoring what a
    /// previous run held and never drained.
    pub fn open(destination: &BacklogDestination) -> Result<Self> {
        Self::open_with_capacity(destination, RESEARCH_BACKLOG_CAPACITY)
    }

    /// [`Self::open`] at `capacity`. Refuses zero, which would refuse every
    /// held round and queue nothing, and a journal holding more pending work
    /// than `capacity`, which would have to be cut to fit.
    pub fn open_with_capacity(destination: &BacklogDestination, capacity: usize) -> Result<Self> {
        if capacity == 0 {
            return Err(Error::invalid(
                "a research backlog of capacity zero refuses every held round; give it room for \
                 at least one",
            ));
        }
        let log = match destination {
            BacklogDestination::InMemory => EventLog::in_memory(),
            BacklogDestination::File(path) => EventLog::open(path)?,
        };
        if let Err(sequence) = log.verify_chain() {
            return Err(Error::invalid(format!(
                "the research backlog's hash chain breaks at sequence {sequence}; move the file \
                 aside rather than extend it"
            )));
        }
        let mut pending = VecDeque::new();
        for event in log.events() {
            if event.lineage.producer != BACKLOG_ORIGIN {
                continue;
            }
            match event.decode::<BacklogRecord>()?.body {
                BacklogRecord::Held { work } => pending.push_back(work),
                BacklogRecord::Drained { cycle, .. } => match pending.pop_front() {
                    Some(work) if work.cycle == cycle => {}
                    _ => {
                        return Err(Error::invalid(format!(
                            "the research backlog records cycle {cycle} drained out of the order \
                             it was held in; move the file aside rather than replay it"
                        )));
                    }
                },
                BacklogRecord::Refused { .. } => {}
            }
        }
        if pending.len() > capacity {
            return Err(Error::invalid(format!(
                "the research backlog holds {} pending rounds against a capacity of {capacity}; \
                 open it with the capacity it was written under",
                pending.len()
            )));
        }
        let written = log.events().last().map_or(0, |e| e.sequence);
        Ok(Self {
            log,
            pending,
            capacity,
            written,
            refused: 0,
        })
    }

    /// Hold `work` until the platform returns. Journaled before it is
    /// queued, so nothing is pending that the log does not show held.
    ///
    /// At capacity the work is refused and the refusal journaled; nothing
    /// already held is evicted to make room.
    pub fn hold(&mut self, work: HeldWork) -> Result<()> {
        if self.pending.len() >= self.capacity {
            let cycle = work.cycle;
            let at = work.held_at;
            self.refused = self.refused.saturating_add(1);
            self.append(
                BacklogRecord::Refused {
                    work,
                    capacity: self.capacity,
                },
                at,
            )?;
            return Err(Error::guard(format!(
                "the research backlog holds {} rounds, its capacity; cycle {cycle}'s held rounds \
                 are refused rather than evicting older ones. They are journaled as refused",
                self.capacity
            )));
        }
        let at = work.held_at;
        self.append(BacklogRecord::Held { work: work.clone() }, at)?;
        self.pending.push_back(work);
        Ok(())
    }

    /// The oldest held work, journaled as drained, or `None` when nothing
    /// waits.
    pub fn drain_next(&mut self, at: Timestamp) -> Result<Option<HeldWork>> {
        let Some(cycle) = self.pending.front().map(|work| work.cycle) else {
            return Ok(None);
        };
        self.append(
            BacklogRecord::Drained {
                cycle,
                drained_at: at,
            },
            at,
        )?;
        Ok(self.pending.pop_front())
    }

    pub fn pending(&self) -> impl Iterator<Item = &HeldWork> {
        self.pending.iter()
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Held work refused at capacity since this backlog was opened.
    pub const fn refused(&self) -> u64 {
        self.refused
    }

    pub fn log(&self) -> &EventLog {
        &self.log
    }

    fn append(&mut self, record: BacklogRecord, at: Timestamp) -> Result<EventId> {
        let event_id = EventId::from_string(format!("evt-backlog-{}", self.written));
        let lineage = Lineage::root(
            CorrelationId::from_string(format!("research-backlog-{}", self.written)),
            BACKLOG_ORIGIN,
        );
        let event = Envelope::new(event_id.clone(), at, at, lineage, record).erase()?;
        self.log.append(&event)?;
        self.written += 1;
        Ok(event_id)
    }
}

/// Every backlog record in `log`, in log order. One that does not decode is
/// refused, not skipped.
pub fn records(log: &EventLog) -> Result<Vec<BacklogRecord>> {
    let mut out = Vec::new();
    for event in log.events() {
        if event.lineage.producer != BACKLOG_ORIGIN {
            continue;
        }
        out.push(event.decode::<BacklogRecord>()?.body);
    }
    Ok(out)
}

/// What the cognition gate did on one cycle, for the cycle line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CognitionRound {
    /// Why the platform was lost on this cycle, when it was.
    pub lost: Option<Unready>,
    /// The loops held on this cycle because they were due.
    pub held: BTreeSet<CognitiveLoop>,
    /// Whether the held loops reached the backlog.
    pub queued: bool,
    /// The cycles whose held work ran on this cycle, in the order drained.
    pub drained: Vec<u64>,
    /// Held work still waiting after this cycle.
    pub pending: usize,
}

impl CognitionRound {
    pub fn describe(&self) -> String {
        let held: Vec<&str> = self.held.iter().map(|l| l.as_str()).collect();
        match self.lost {
            Some(cause) => format!(
                "cognition held: the AI platform is lost ({}); held [{}]{}; {} round(s) waiting",
                cause.as_str(),
                held.join(", "),
                if self.queued || held.is_empty() {
                    ""
                } else {
                    " and NOT queued"
                },
                self.pending
            ),
            None => format!(
                "cognition resumed: drained held cycle(s) {:?}; {} waiting",
                self.drained, self.pending
            ),
        }
    }

    /// Whether there is anything to print.
    pub fn is_quiet(&self) -> bool {
        self.lost.is_none() && self.drained.is_empty()
    }
}
