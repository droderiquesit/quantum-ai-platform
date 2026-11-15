//! The event-log record of what a scout pass found about each candidate
//! (R12-DATA-1102 novelty, R12-DATA-1103 access rights; ADR 0626).
//!
//! `SourceLifecycleChanged` is written only when policy moves a registration
//! that already stands. A first-seen source refused for its robots rules or
//! its licence moved nothing, so the refusal — the platform's whole answer to
//! "may this be read?" — lived in the returned decisions and nowhere durable,
//! and the novelty the finder scored was never recorded for anything. A
//! replay could not say which sources had been looked at, let alone why one
//! was turned away.
//!
//! Each check can be switched off per pass ([`ScoutChecks`]). A disabled
//! check records `None`, never a default verdict: an access-rights field
//! reading "permitted" because nobody asked would be a licence the platform
//! never read, and it is what makes disabling a check change the recorded
//! outcome rather than only the work done.

use crate::platform::Platform;
use qip_core::Timestamp;
use qip_core::error::Result;
use qip_data_finder::RegistrationDecision;
use qip_data_finder::legal::Legality;
use qip_events::{EventBody, EventFilter, Topic};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Which scout checks a pass records. Both on is the only default a
/// composition root should pick; the switches exist so a pass that skips one
/// says so on the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScoutChecks {
    pub novelty: bool,
    pub access_rights: bool,
}

impl ScoutChecks {
    pub const ALL: Self = Self {
        novelty: true,
        access_rights: true,
    };
}

/// The three access questions the finder asked, and their combination.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessRights {
    pub host: Legality,
    pub robots: Legality,
    pub licensing: Legality,
    pub overall: Legality,
    /// Whether the overall verdict permits use. Stored beside the verdict so
    /// a reader filtering the log need not match on the enum.
    pub permitted: bool,
}

/// One candidate's scout findings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScoutVerdict {
    pub source_id: String,
    /// The registry held no registration under this id when the pass began.
    pub first_pass: bool,
    /// `None` when the check was disabled, or when the finder reached no
    /// legality question (a quarantined source is answered unprobed).
    pub access: Option<AccessRights>,
    /// The finder's uniqueness score in `[0, 1]`: how much of the source is
    /// not already covered. `None` when disabled or never scored.
    pub novelty: Option<f64>,
}

/// One pass's findings, candidates in the finder's own (id) order.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceScouted {
    pub at: Timestamp,
    pub checks_access_rights: bool,
    pub checks_novelty: bool,
    pub verdicts: Vec<ScoutVerdict>,
}

impl EventBody for SourceScouted {
    const TOPIC: Topic = Topic::SourceScouted;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        let ids: Vec<&str> = self.verdicts.iter().map(|v| v.source_id.as_str()).collect();
        Some(format!(
            "source-scouted:{}:{}",
            self.at.as_nanos(),
            ids.join(",")
        ))
    }
}

impl Platform {
    /// Journal one pass's findings. Raised on a journal refusal: the finder
    /// has already decided, and a verdict the log does not hold is the gap
    /// this record exists to close. Writes nothing for an empty pass.
    pub(crate) fn journal_scout_verdicts(
        &mut self,
        decisions: &[RegistrationDecision],
        held_before: &BTreeSet<String>,
        checks: ScoutChecks,
        now: Timestamp,
    ) -> Result<()> {
        if decisions.is_empty() {
            return Ok(());
        }
        let verdicts = decisions
            .iter()
            .map(|decision| ScoutVerdict {
                source_id: decision.source_id().to_string(),
                first_pass: !held_before.contains(decision.source_id()),
                access: checks
                    .access_rights
                    .then(|| decision.legality())
                    .flatten()
                    .map(|legal| AccessRights {
                        host: legal.host().clone(),
                        robots: legal.robots().clone(),
                        licensing: legal.licensing().clone(),
                        overall: legal.overall().clone(),
                        permitted: matches!(legal.overall(), Legality::Permitted { .. }),
                    }),
                novelty: checks
                    .novelty
                    .then(|| decision.scores().map(|scores| scores.uniqueness()))
                    .flatten(),
            })
            .collect();
        self.journal_once(
            SourceScouted {
                at: now,
                checks_access_rights: checks.access_rights,
                checks_novelty: checks.novelty,
                verdicts,
            },
            "kernel/scout",
            now,
        )?;
        Ok(())
    }

    /// Every scout pass the log holds, in log order.
    pub fn scout_passes(&self) -> Result<Vec<SourceScouted>> {
        self.replay_journal(&EventFilter::new().topic(Topic::SourceScouted))?
            .iter()
            .map(|envelope| Ok(envelope.decode::<SourceScouted>()?.body))
            .collect()
    }
}
