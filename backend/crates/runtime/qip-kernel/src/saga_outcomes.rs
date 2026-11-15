//! One record per multileg saga that did not complete (R12-EXPAND-6066,
//! ADR 0193 decision 1).
//!
//! A leg group that ends unwound, abandoned or escalated is a failed
//! arbitrage: the platform wanted two things to happen together and one did
//! not. `LegGroup` holds the facts in its own state, and nothing carried them
//! out of it, so the curriculum had no input to raise a gap on and an
//! operator could not ask afterwards which venue kept missing legs. The
//! platform journals one bounded record per failed group, and the deep
//! brain's sweep reads them back off the log as it does the reconciliation
//! breaks. A group that completed is not a failure and leaves no record.
//!
//! The record is shared with the break record's topic and told apart by its
//! key prefix, as `central/learning.rs`'s disposition record already is.

use crate::platform::Platform;
use qip_core::error::Result;
use qip_core::{Decimal, Timestamp};
use qip_events::{EventBody, Topic};
use qip_execution_engine::multileg::{GroupState, LegGroup};
use qip_streaming::envelope::StreamEnvelope;
use serde::{Deserialize, Serialize};

/// The lineage origin saga records are journaled under.
pub const SAGA_OUTCOME_ORIGIN: &str = "kernel/saga-outcome";

/// What every saga record's idempotency key starts with.
pub const SAGA_OUTCOME_PREFIX: &str = "saga-outcome:";

/// One saga that did not complete.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SagaOutcomeRecord {
    /// `<venue>:<group id>`: one record per group per venue, so a replay of
    /// the same group dedupes.
    pub record_id: String,
    pub venue: String,
    pub group_id: String,
    pub legs_placed: usize,
    /// Legs that filled any quantity.
    pub legs_filled: usize,
    /// The first leg that did not fill its full quantity.
    pub failed_leg: Option<String>,
    /// Reversing orders issued to close what had filled.
    pub unwind_orders: usize,
    /// `GroupState::as_str` of the end the group reached.
    pub end: String,
    pub reason: String,
    /// Quantity still open when it ended; zero unless it escalated.
    pub residual_quantity: Decimal,
    pub at: Timestamp,
}

impl EventBody for SagaOutcomeRecord {
    const TOPIC: Topic = Topic::ReconciliationCompleted;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{SAGA_OUTCOME_PREFIX}{}", self.record_id))
    }
}

/// The record a group's end makes, or `None` for a group that has not
/// ended, or ended complete. Refuses an empty venue, because a gap per venue
/// cannot be raised against a name nobody gave.
pub fn record_of(
    venue: &str,
    group: &LegGroup,
    now: Timestamp,
) -> Result<Option<SagaOutcomeRecord>> {
    if venue.trim().is_empty() {
        return Err(qip_core::error::Error::invalid(
            "a saga outcome needs the venue it ran at; the curriculum raises one gap per venue",
        ));
    }
    let (reason, residual) = match &group.state {
        GroupState::Closed { .. } => (
            "the legs did not complete and were reversed".to_string(),
            Decimal::ZERO,
        ),
        GroupState::Abandoned { reason, .. } => (reason.clone(), Decimal::ZERO),
        GroupState::Escalated {
            reason,
            residual_quantity,
            ..
        } => (reason.clone(), *residual_quantity),
        _ => return Ok(None),
    };
    Ok(Some(SagaOutcomeRecord {
        record_id: format!("{venue}:{}", group.group_id),
        venue: venue.to_string(),
        group_id: group.group_id.clone(),
        legs_placed: group.legs.len(),
        legs_filled: group
            .legs
            .iter()
            .filter(|leg| leg.filled.is_positive())
            .count(),
        failed_leg: group
            .legs
            .iter()
            .find(|leg| leg.filled < leg.order.quantity)
            .map(|leg| leg.order.order_id.to_string()),
        unwind_orders: group.unwinds().len(),
        end: group.state.as_str().to_string(),
        reason,
        residual_quantity: residual,
        at: now,
    }))
}

impl Platform {
    /// Journal the outcome of a leg group that has ended without completing.
    /// Returns whether a record was written; a group still working, one
    /// that completed, or one already recorded writes none. A journal failure is returned, not
    /// swallowed: the caller that ran the saga decides whether an unrecorded
    /// failure is a halt.
    pub fn record_saga_outcome(
        &mut self,
        venue: &str,
        group: &LegGroup,
        now: Timestamp,
    ) -> Result<bool> {
        let Some(record) = record_of(venue, group, now)? else {
            return Ok(false);
        };
        // The log does not drop a second frame under the same key, so a
        // replayed group would otherwise be counted twice by the sweep.
        if self
            .saga_outcome_records()?
            .iter()
            .any(|held| held.record_id == record.record_id)
        {
            return Ok(false);
        }
        self.journal_record(record, SAGA_OUTCOME_ORIGIN, now)?;
        Ok(true)
    }

    /// Every saga outcome record the log holds, oldest first, read back as
    /// `reconciliation_break_records` is.
    pub fn saga_outcome_records(&self) -> Result<Vec<SagaOutcomeRecord>> {
        self.event_log()
            .by_topic(<SagaOutcomeRecord as EventBody>::TOPIC)
            .into_iter()
            .filter(|frame| {
                frame
                    .idempotency_key
                    .as_deref()
                    .is_some_and(|key| key.starts_with(SAGA_OUTCOME_PREFIX))
            })
            .map(|frame| {
                let envelope = StreamEnvelope::from_frame(frame)?;
                Ok(envelope.decode::<SagaOutcomeRecord>()?.body)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PlatformConfig;
    use qip_core::Context;
    use qip_core::dec;
    use qip_core::ids::{ObjectId, OrderId};
    use qip_execution_engine::order::{Order, OrderType, Side};
    use qip_financial::universe::Universe;
    use qip_observability::Telemetry;
    use qip_risk::limits::LimitSet;

    fn at(offset: i64) -> Timestamp {
        Timestamp::from_secs(1_760_000_000 + offset)
    }

    fn platform() -> Platform {
        let config = PlatformConfig::default();
        let (context, _clock) = Context::deterministic(at(0), config.seed);
        Platform::new(
            config,
            context,
            Telemetry::silent(),
            Universe::new(),
            LimitSet::new("saga-outcome-test"),
        )
        .expect("a platform")
    }

    fn order(id: &str, side: Side) -> Order {
        Order::new(
            OrderId::from_string(id),
            ObjectId::from_string("obj-A"),
            side,
            dec!("10"),
            OrderType::Market,
            dec!("100"),
            "prop",
            vec!["hyp".to_string()],
            "platform",
            at(0),
        )
    }

    fn group(id: &str) -> LegGroup {
        LegGroup::new(
            id,
            vec![
                order(&format!("{id}-1"), Side::Buy),
                order(&format!("{id}-2"), Side::Sell),
            ],
            at(60),
            dec!("1000000"),
        )
        .expect("a group")
    }

    /// A group that reached its deadline with nothing filled, settled by the
    /// verdict the group itself gives.
    fn abandoned(id: &str) -> LegGroup {
        let mut group = group(id);
        let verdict = group.assess(at(61));
        group.settle(&verdict, &[], at(61)).expect("settles");
        assert_eq!(group.state.as_str(), "abandoned", "premise");
        group
    }

    /// The failure prevented: a failed saga leaves no trace outside the
    /// group's own state, so the curriculum cannot ask which venue misses
    /// legs. Mutation: make `record_of` return `Ok(None)` for `Abandoned`;
    /// the `len() == 1` assertion fails.
    #[test]
    fn a_failed_saga_is_journaled_once_and_read_back_naming_its_venue_and_failed_leg() {
        let mut platform = platform();
        let group = abandoned("g1");
        assert!(
            platform
                .record_saga_outcome("XNYS", &group, at(62))
                .expect("journals")
        );
        // A replay of the same group is not journaled again.
        assert!(
            !platform
                .record_saga_outcome("XNYS", &group, at(63))
                .expect("replay is accepted")
        );
        let records = platform.saga_outcome_records().expect("decodes");
        assert_eq!(records.len(), 1, "{records:?}");
        let record = &records[0];
        assert_eq!(record.venue, "XNYS");
        assert_eq!(record.group_id, "g1");
        assert_eq!(record.legs_placed, 2);
        assert_eq!(record.legs_filled, 0);
        assert_eq!(record.failed_leg.as_deref(), Some("g1-1"));
        assert_eq!(record.end, "abandoned");
    }

    /// A group still working, or one that has not ended, is no failure.
    /// Mutation: drop the `_ => return Ok(None)` arm for `return` of a
    /// record; this fails.
    #[test]
    fn a_group_that_has_not_ended_writes_no_record() {
        let mut platform = platform();
        let pending = group("g2");
        assert!(
            !platform
                .record_saga_outcome("XNYS", &pending, at(1))
                .expect("no error")
        );
        assert!(platform.saga_outcome_records().expect("decodes").is_empty());
    }

    #[test]
    fn a_saga_with_no_venue_is_refused_rather_than_filed_under_nothing() {
        let mut platform = platform();
        let error = platform
            .record_saga_outcome("  ", &abandoned("g3"), at(62))
            .expect_err("refused");
        assert!(error.message().contains("venue"), "{}", error.message());
    }
}
