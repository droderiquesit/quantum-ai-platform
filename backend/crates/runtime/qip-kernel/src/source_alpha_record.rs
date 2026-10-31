//! The event-log record of one source-scoring round (R12-EVID-8108).
//!
//! The deep brain scores each connector it senses on incremental information
//! gain over what the platform already holds and on realised contribution to
//! the outcome that followed (`qip_data_finder::source_alpha`). The history
//! it keeps is in memory and bounded, so before this record a restarted node
//! had no scores, and a source promoted or retired on its score left nothing
//! a replay could read back to explain why.

use crate::platform::Platform;
use qip_core::Timestamp;
use qip_core::error::Result;
use qip_data_finder::source_alpha::SourceAlpha;
use qip_events::{EventBody, EventFilter, Topic};
use serde::{Deserialize, Serialize};

/// One source scored on one subject, or why it could not be.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoredSource {
    pub source: String,
    pub subject: String,
    /// Aligned samples the score was computed over.
    pub samples: usize,
    /// The score, or the refusal's message.
    pub outcome: std::result::Result<SourceAlpha, String>,
}

/// Every pair one round scored, in the order the desk scored them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceAlphaScored {
    /// The research cycle the round ran on.
    pub cycle: u64,
    pub at: Timestamp,
    pub scores: Vec<ScoredSource>,
}

impl EventBody for SourceAlphaScored {
    const TOPIC: Topic = Topic::SourceAlphaScored;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "source-alpha:{}:{}",
            self.cycle,
            self.at.as_nanos()
        ))
    }
}

impl Platform {
    /// Journal one scoring round. Returns whether it was written; a round
    /// the log already holds for the same cycle and instant is not written
    /// twice. A refusal is raised: a round the desk scored and the log does
    /// not hold is a score no replay can reproduce.
    pub fn journal_source_alpha_round(&mut self, round: SourceAlphaScored) -> Result<bool> {
        let at = round.at;
        self.journal_once(round, "kernel/source-alpha", at)
    }

    /// Every scoring round the log holds, in log order.
    pub fn source_alpha_rounds(&self) -> Result<Vec<SourceAlphaScored>> {
        self.replay_journal(&EventFilter::new().topic(Topic::SourceAlphaScored))?
            .iter()
            .map(|envelope| Ok(envelope.decode::<SourceAlphaScored>()?.body))
            .collect()
    }
}
