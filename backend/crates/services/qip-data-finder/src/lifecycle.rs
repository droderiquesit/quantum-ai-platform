//! What re-assessment does to a source the registry already holds
//! (DATA-018, DATA-021).
//!
//! The finder re-derives every decision from fresh evidence each pass, and
//! until this module that was a statement about *candidates* only. A source
//! registered on Monday whose publisher forbade the path on Tuesday was
//! rejected as a candidate on Tuesday's pass and left standing in the
//! registry — still "currently collected" to everything that reads it —
//! because a rejection wrote nothing. Its terms had been re-read and the
//! answer thrown away.
//!
//! The policy is one pure function: what the registry held, what the pass
//! found, and the action that follows. No clock, no I/O, no operator. It
//! decides nothing about legality or score — [`crate::finder::DataFinder`]
//! already did — only what a changed answer means for a standing
//! registration.

use crate::scoring::RoutingClass;
use qip_core::Timestamp;
use serde::{Deserialize, Serialize};

/// What policy did to a registered source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleAction {
    /// Its score rose into a more eagerly polled class.
    Promoted,
    /// Its score fell into a less eagerly polled class. It is still
    /// collected, less often.
    Throttled,
    /// Use is no longer permitted, or the source can no longer be trusted.
    /// The registration is kept, marked, and the source is not contacted
    /// again: the record of *why* it stopped has to outlive the stopping.
    Quarantined,
    /// Use is permitted and the source is no longer worth collecting. The
    /// registration is removed.
    Retired,
}

impl LifecycleAction {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Promoted => "promoted",
            Self::Throttled => "throttled",
            Self::Quarantined => "quarantined",
            Self::Retired => "retired",
        }
    }
}

/// One move of one registered source, with what it moved between and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LifecycleTransition {
    pub source_id: String,
    pub action: LifecycleAction,
    /// The class the registry held before the pass.
    pub from: RoutingClass,
    /// The class after it. [`RoutingClass::Rejected`] for a quarantine or a
    /// retirement, which both end collection.
    pub to: RoutingClass,
    pub reason: String,
    pub at: Timestamp,
}

/// What a pass found about a source the registry holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reassessment<'a> {
    /// Still collectable, in this class.
    Collected(RoutingClass),
    /// Use is permitted and the composite score is under the collection
    /// floor.
    BelowFloor { reason: &'a str },
    /// Refused for anything other than its score: terms, robots, the host
    /// rules, personal data, manipulation, registration standing.
    Refused { reason: &'a str },
    /// The probe could not reach it. That is a fact about this platform's
    /// reach, not about the source, and it moves nothing — a crawler outage
    /// that retired every source it could not reach would be an outage that
    /// deleted the catalogue.
    Unreached,
}

/// The transition a finding implies for a source held in `held`, if any.
///
/// `held` is never [`RoutingClass::Rejected`] for a registered source; if it
/// is passed anyway nothing can be "more eager" than it and the function
/// reports a promotion to whatever was found, which is the truthful reading.
pub fn policy(
    source_id: &str,
    held: RoutingClass,
    found: Reassessment<'_>,
    at: Timestamp,
) -> Option<LifecycleTransition> {
    let (action, to, reason) = match found {
        Reassessment::Unreached => return None,
        Reassessment::Collected(now) if now == held => return None,
        // `RoutingClass` orders most eager first: Hot < Warm < Cold.
        Reassessment::Collected(now) if now < held => (
            LifecycleAction::Promoted,
            now,
            format!("re-scored from {} to {}", held.as_str(), now.as_str()),
        ),
        Reassessment::Collected(now) => (
            LifecycleAction::Throttled,
            now,
            format!("re-scored from {} to {}", held.as_str(), now.as_str()),
        ),
        Reassessment::BelowFloor { reason } => (
            LifecycleAction::Retired,
            RoutingClass::Rejected,
            reason.to_string(),
        ),
        Reassessment::Refused { reason } => (
            LifecycleAction::Quarantined,
            RoutingClass::Rejected,
            reason.to_string(),
        ),
    };
    Some(LifecycleTransition {
        source_id: source_id.to_string(),
        action,
        from: held,
        to,
        reason,
        at,
    })
}
