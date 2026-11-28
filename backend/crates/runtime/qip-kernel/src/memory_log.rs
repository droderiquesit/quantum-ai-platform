//! The byte ceiling on a platform log held only in memory (ADR 0748).
//!
//! A platform whose event log is [`crate::EventLogDestination::InMemory`]
//! holds every record twice — once in its `EventLog`, once in the kernel
//! journal that mirrors it — and nothing else bounded either by size. The
//! fast brain on its committed `memory` store grew past its 2 GiB container
//! in minutes, about thirteen hours before the first material release on its
//! committed seed, so a deployed node never met one.
//!
//! [`crate::Platform::bound_memory_log`] states a ceiling. At the end of each
//! cycle, once the two logs together hold more than it, each releases its
//! oldest records as one whole range down to a quarter of the ceiling — half
//! of it between the two — and the platform appends an
//! [`EventLogCheckpointed`] to both, carrying each range's last hash and
//! record count. From then on each log is verified from its checkpoint
//! forward ([`qip_events::EventLog::verify_from_checkpoint`]) and
//! `replay_journal` returns what is retained after it.
//!
//! **Said out loud: on memory storage this log no longer replays from
//! genesis.** It replays from the newest checkpoint. Nothing durable changes:
//! a memory log never survived a restart, and a log written to a file is
//! never released — `release_through` refuses it.
//!
//! What a replay from the checkpoint needs that the released range held is
//! carried on the checkpoint, not guessed: the federation journal writes an
//! unchanged arbitration as a reference to the pass that last held it in full
//! (`JournaledReview` v2), up to 99 passes back, so a retained review may
//! name a pass in the released range. The checkpoint carries those bases,
//! and `federation_stage::replay_each` starts from them.

use qip_events::log::ReleasedRange;
use qip_events::{EventBody, Topic};
use qip_world_model::federation::Arbitration;
use serde::{Deserialize, Serialize};

/// The producer every checkpoint is journaled under.
pub const MEMORY_LOG_ORIGIN: &str = "kernel/memory-log";

/// The checkpoint appended after a release: what each log released, and what
/// a replay from here needs that the released records held.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventLogCheckpointed {
    /// The ceiling the release kept the two logs under, in estimated bytes.
    pub ceiling_bytes: u64,
    /// What the two logs held, together, when the release ran.
    pub held_bytes: u64,
    /// The range the platform's event log released.
    pub event_log: ReleasedRange,
    /// The range the kernel journal released.
    pub journal: ReleasedRange,
    /// The federation journal's bases as of the journal's released range:
    /// stem, the pass that held it in full, and that arbitration.
    pub federation_bases: Vec<(String, u64, Arbitration)>,
}

impl EventBody for EventLogCheckpointed {
    const TOPIC: Topic = Topic::EventLogCheckpointed;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("memory-log:{}", self.journal.through))
    }
}

/// The ceiling a platform keeps its two in-memory logs under, and the
/// federation bases the last checkpoint carried, which the next one advances.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MemoryLogBound {
    pub(crate) ceiling_bytes: u64,
    pub(crate) bases: crate::federation_stage::JournalBases,
}
