//! The executable scope, applied where opportunities enter the queue
//! (R12-GOV-6117, R12-GOV-6122).
//!
//! `qip_contracts::action::ExecutableScope::route` is the structural half:
//! an opportunity on an instrument the scope does not name becomes a
//! [`qip_contracts::action::ShadowRecord`], a type with no conversion to an
//! [`qip_contracts::action::ActionIntent`]. Until this module the kernel
//! never called it, so every detected opportunity — on an instrument the
//! desk holds limits and a liquidity profile for, or on one it has only ever
//! seen a price series for — went into the same queue and on to REASON. The
//! scope is the universe the platform was assembled with: the instruments
//! that have a liquidity profile for the sizing controls to read and a
//! venue for settlement to be joined to. A detection on anything else is
//! recorded on the journal as shadowed and never queued, so however it is
//! rescored on a later pass it is re-detected, re-routed and re-shadowed —
//! the route takes no score, so a higher one cannot promote it.
//!
//! An opportunity that names no instrument at all — an entity-level
//! detection, which cannot become an order on its own — is not routed here:
//! the scope is a bound on capital actions, and such an opportunity has no
//! instrument to be in or out of scope on. It keeps the path it had.

use crate::platform::Platform;
use qip_contracts::action::{ExecutableScope, Routed};
use qip_core::Timestamp;
use qip_core::error::Result;
use qip_events::{EventBody, Topic};
use qip_opportunity_engine::opportunity::Opportunity;
use serde::{Deserialize, Serialize};

/// The journal record of an opportunity the scope refused to queue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpportunityShadowed {
    pub opportunity_id: String,
    pub instrument: String,
    pub headline: String,
    pub detected_at: Timestamp,
    pub shadowed_at: Timestamp,
}

impl EventBody for OpportunityShadowed {
    const TOPIC: Topic = Topic::OpportunityRanked;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        // One record per detection, not per pass: a re-detection carries a
        // new opportunity id and is journaled again, which is the fact the
        // operator wants — the detector keeps finding it and the scope keeps
        // refusing it.
        Some(format!("opportunity-shadowed:{}", self.opportunity_id))
    }
}

/// What routing one pass's detections produced.
pub(crate) struct Routing {
    /// The opportunities the scope admits, in detection order.
    pub(crate) executable: Vec<Opportunity>,
    /// How many were shadowed.
    pub(crate) shadowed: usize,
    /// Journal refusals, one line each. A shadow the log does not hold is
    /// an opportunity that vanished, so each is raised on the stage.
    pub(crate) problems: Vec<String>,
}

impl Platform {
    /// The executable scope: the instruments the platform was assembled with.
    pub fn executable_scope(&self) -> ExecutableScope {
        ExecutableScope::new(
            self.market_view()
                .universe
                .ids()
                .map(|id| id.as_str().to_string()),
        )
    }

    /// Route every detected opportunity through the executable scope,
    /// journaling the ones it shadows. See the module documentation for
    /// which opportunities are routed at all.
    pub(crate) fn route_detected(&mut self, found: Vec<Opportunity>, now: Timestamp) -> Routing {
        let scope = self.executable_scope();
        let mut routing = Routing {
            executable: Vec::with_capacity(found.len()),
            shadowed: 0,
            problems: Vec::new(),
        };
        for opportunity in found {
            let Some(instrument) = opportunity.affected_objects.first() else {
                routing.executable.push(opportunity);
                continue;
            };
            match scope.route(opportunity.opportunity_id.as_str(), instrument.as_str()) {
                Routed::Executable(_) => routing.executable.push(opportunity),
                Routed::Shadow(shadow) => {
                    routing.shadowed += 1;
                    if let Err(error) = self.journal_shadow(&opportunity, shadow.instrument(), now)
                    {
                        routing.problems.push(format!(
                            "opportunity {} on {} was shadowed and the journal refused the record: {}",
                            opportunity.opportunity_id.as_str(),
                            instrument,
                            error.message()
                        ));
                    }
                }
            }
        }
        routing
    }

    fn journal_shadow(
        &mut self,
        opportunity: &Opportunity,
        instrument: &str,
        now: Timestamp,
    ) -> Result<()> {
        self.journal_once(
            OpportunityShadowed {
                opportunity_id: opportunity.opportunity_id.as_str().to_string(),
                instrument: instrument.to_string(),
                headline: opportunity.headline.clone(),
                detected_at: opportunity.detected_at,
                shadowed_at: now,
            },
            "kernel/executable-scope",
            now,
        )?;
        Ok(())
    }
}
