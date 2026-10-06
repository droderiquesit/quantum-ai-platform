//! The event-log record of policy moving a registered source (DATA-018).
//!
//! `qip-data-finder` decides the move and carries it on the decision; the
//! registry it mutates is in memory and holds only the present. Without this
//! record a source retired on Tuesday is, by Wednesday, a source that was
//! never registered: the pass that stopped reading it left nothing a replay
//! could find, and "why did we stop" had no answer anywhere durable.

use crate::platform::Platform;
use qip_core::Timestamp;
use qip_core::error::Result;
use qip_data_finder::{LifecycleTransition, RegistrationDecision};
use qip_events::{EventBody, Topic};
use serde::{Deserialize, Serialize};

/// One transition, as the finder's policy produced it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLifecycleChanged {
    pub transition: LifecycleTransition,
}

impl EventBody for SourceLifecycleChanged {
    const TOPIC: Topic = Topic::SourceLifecycleChanged;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "source-lifecycle:{}:{}:{}",
            self.transition.source_id,
            self.transition.action.as_str(),
            self.transition.at.as_nanos()
        ))
    }
}

impl Platform {
    /// Journal every transition `decisions` carry. A journal refusal is
    /// raised: the registry has already moved, and a move the log does not
    /// hold is the one outcome this record exists to prevent, so the pass
    /// must not report success.
    pub(crate) fn journal_source_transitions(
        &mut self,
        decisions: &[RegistrationDecision],
        now: Timestamp,
    ) -> Result<()> {
        for transition in decisions
            .iter()
            .filter_map(RegistrationDecision::transition)
        {
            self.journal_once(
                SourceLifecycleChanged {
                    transition: transition.clone(),
                },
                "kernel/sources",
                now,
            )?;
        }
        Ok(())
    }
}
