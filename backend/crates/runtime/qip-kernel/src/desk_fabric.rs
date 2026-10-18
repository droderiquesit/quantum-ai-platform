//! The desk's ledger records, as the event-fabric records `qip-ledgerd` reads.
//!
//! [`crate::desk_ledger`] makes the `OutcomeRecord`; this makes the P1
//! `reflex.outcome_recorded` [`Record`] that carries it to the ledger. It is
//! the desk's half of what `qip-edge-node`'s spool writer does for a cell's
//! fills, and it is deliberately **not** a second envelope scheme: the topic
//! binding, the schema version, the canonical-JSON payload and its hash, the
//! event id's shape and the lineage rule are the edge writer's, so the one
//! consumer (`qip-ledgerd::consumer::decode_batch`) has one thing to read.
//!
//! The edge writer lives in an app crate the kernel may not depend on, so the
//! two are kept equal by a test instead of by a shared function:
//! `qip-acceptance`'s `desk_record_parity` builds the same `OutcomeRecord`
//! through both and asserts every field the ledger consumes is identical.
//! What may legitimately differ is named there (the lineage's `producer`
//! string, which says who wrote the record and which the ledger does not
//! read).
//!
//! # What the ledger will refuse, refused here first
//!
//! `decode_batch` requires the lineage correlation to equal the id the body
//! derives, and the ledger parks the desk's whole chain on a fill that is not
//! marked simulated. A record the ledger would refuse is acknowledged by the
//! broker all the same and then stalls the partition for ever, because the
//! ledger never skips a record it cannot read. So [`fabric_record`] refuses
//! such a record where the desk can say no: a body that is not a simulated
//! fill is [`Error::denied`], which also keeps the paper boundary structural
//! at this seam — a live fill cannot be built into a record that ships.

use crate::desk_ledger::{DESK_CELL, DESK_SESSION, DeskOutcome, validate_chain};
use qip_contracts::ledger_operation::LedgerOperation;
use qip_contracts::reflex::Decision;
use qip_core::canonical::canonical_json;
use qip_core::error::{Error, Result};
use qip_core::{EventId, Lineage, sha256_hex};
use qip_events::AnyEvent;
use qip_events::event_fabric::bindings::{LEDGER_OPERATION_RECORDED, REFLEX_OUTCOME_RECORDED};
use qip_events::event_fabric::codec::{Batch, MessageType, PayloadCodec, Record};
use qip_events::event_fabric::policy::{BATCH_SCHEMA_VERSION, QosClass};
use qip_portfolio::ledger::{PaperFill, post};

/// Who wrote the record, for the lineage. Not read by the ledger.
pub const PRODUCER: &str = "qip-kernel.desk";

/// The stream-kind component of an event id, the edge writer's
/// `RecordKind::OutcomeRecorded.as_str()`.
const KIND: &str = "p1.outcome_recorded";

/// The event id of the desk's record at chain `sequence`:
/// `sha256(cell | session | sequence | kind)`, the edge writer's `event_id`
/// for a record of kind `OutcomeRecorded` — a function of the fill's place on
/// the chain and nothing else, so a resend after a crash carries the id the
/// first send did and the broker's and ledger's duplicate rules apply.
pub fn event_id(sequence: u64) -> String {
    event_id_on(DESK_CELL, sequence)
}

/// [`event_id`] on the named desk chain.
pub fn event_id_on(chain: &str, sequence: u64) -> String {
    sha256_hex(format!("{chain}|{DESK_SESSION}|{sequence}|{KIND}").as_bytes())
}

/// The fabric record for one desk outcome, or the reason the ledger would
/// not take it.
pub fn fabric_record(outcome: &DeskOutcome) -> Result<Record> {
    let record = &outcome.record;
    if validate_chain(&record.cell).is_err() || record.session != DESK_SESSION {
        return Err(Error::invalid(format!(
            "a desk outcome names chain {}/{}; a desk ships only a desk chain at session \
             {DESK_SESSION}",
            record.cell, record.session
        )));
    }
    if record.journal_sequence != record.entry.sequence
        || record.journal_digest != record.entry.digest
    {
        return Err(Error::invalid(format!(
            "desk outcome {} names a journal position other than its entry's; the ledger would \
             park the chain on it",
            record.entry.sequence
        )));
    }
    match &record.entry.decision {
        Decision::Filled {
            simulated: true, ..
        } => {}
        Decision::Filled {
            simulated: false, ..
        } => {
            return Err(Error::denied(format!(
                "desk outcome {} is a fill not marked simulated; this platform trades on paper \
                 only and the ledger parks the chain on such a fill. It is not shipped",
                record.entry.sequence
            )));
        }
        _ => {
            return Err(Error::invalid(format!(
                "desk outcome {} is not a fill; the desk's feed emits fills only",
                record.entry.sequence
            )));
        }
    }
    // The ledger's own two steps, so a record built here posts there.
    let fill = PaperFill::try_from(record)?;
    post(&fill)?;
    let chain = record.correlation().ok_or_else(|| {
        Error::invalid(format!(
            "desk outcome {} derives no chain correlation, so no lineage the ledger accepts",
            record.entry.sequence
        ))
    })?;

    let binding = REFLEX_OUTCOME_RECORDED;
    let payload = serde_json::to_value(record)
        .map_err(|error| Error::schema(format!("a desk outcome would not serialise: {error}")))?;
    let payload_hash = sha256_hex(canonical_json(&payload).as_bytes());
    let id = event_id_on(&record.cell, record.entry.sequence);
    let event = AnyEvent {
        event_id: EventId::from_string(id.clone()),
        topic: binding.topic,
        schema_version: binding.schema_version,
        occurred_at: record.entry.at,
        recorded_at: record.entry.at,
        sequence: 0,
        // The chain id, not the cycle's: the ledger refuses a record whose
        // lineage is not the id its body derives. The cycle that placed the
        // order stays on [`DeskOutcome::correlation_id`].
        lineage: Lineage {
            correlation_id: chain,
            causation_id: None,
            trace_id: None,
            producer: PRODUCER.to_string(),
        },
        idempotency_key: Some(id),
        payload,
        payload_hash,
    };
    Record::from_any_event(&event, PayloadCodec::CanonicalJson)
}

/// The fabric record for one of the desk's ledger operations (ADR 0123): the
/// operation as the body, its event id and lineage both derived from its
/// operation id, so a resend after a crash carries the id the first send did.
pub fn operation_record(operation: &LedgerOperation) -> Result<Record> {
    let binding = LEDGER_OPERATION_RECORDED;
    let payload = serde_json::to_value(operation).map_err(|error| {
        Error::schema(format!(
            "a desk ledger operation would not serialise: {error}"
        ))
    })?;
    let payload_hash = sha256_hex(canonical_json(&payload).as_bytes());
    let id = sha256_hex(format!("qip.ledger.operation.v1|{}", operation.operation_id).as_bytes());
    let event = AnyEvent {
        event_id: EventId::from_string(id.clone()),
        topic: binding.topic,
        schema_version: binding.schema_version,
        occurred_at: operation.recorded_at,
        recorded_at: operation.recorded_at,
        sequence: 0,
        lineage: Lineage {
            correlation_id: qip_core::CorrelationId::from_string(id.clone()),
            causation_id: None,
            trace_id: None,
            producer: PRODUCER.to_string(),
        },
        idempotency_key: Some(id),
        payload,
        payload_hash,
    };
    Record::from_any_event(&event, PayloadCodec::CanonicalJson)
}

/// Records for `outcomes`, in order, or the first refusal with its position.
pub fn fabric_records(outcomes: &[DeskOutcome]) -> Result<Vec<Record>> {
    outcomes.iter().map(fabric_record).collect()
}

/// A P1 batch of `records`, stamped as the edge writer stamps one: the P1
/// schema id and the current batch schema version. The sequence, producer
/// and epoch are the producer's to stamp at send.
pub fn p1_batch(records: Vec<Record>) -> Result<Batch> {
    Batch::new(
        MessageType::Data,
        QosClass::P1Outcomes.batch_schema_id(),
        BATCH_SCHEMA_VERSION,
        PayloadCodec::CanonicalJson,
        records,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desk_ledger::{Admission, DeskLedgerFeed};
    use crate::platform::FillBooked;
    use qip_core::{Timestamp, dec};
    use qip_execution_engine::order::Side;

    fn outcome(sequence_orders: &[&str]) -> Vec<DeskOutcome> {
        let mut feed = DeskLedgerFeed::new();
        for order in sequence_orders {
            let booked = FillBooked {
                order_id: (*order).to_string(),
                fill_id: "f1".to_string(),
                object_id: "obj-ACME".to_string(),
                venue: "paper-xnys".to_string(),
                side: Side::Buy,
                quantity: dec!("10"),
                price: dec!("100"),
                costs: dec!("1.5"),
                at: Timestamp::from_secs(1_760_000_100),
                simulated: true,
            };
            assert!(matches!(
                feed.admit(&booked, "central-desk", "cycle-corr").unwrap(),
                Admission::Recorded { .. }
            ));
        }
        feed.pending().to_vec()
    }

    #[test]
    fn a_desk_record_carries_the_chain_correlation_the_ledger_requires() {
        // The failure this prevents: the cycle's id on the wire, which the
        // ledger refuses and then stalls its partition behind.
        let outcomes = outcome(&["o1"]);
        assert_eq!(outcomes[0].correlation_id, "cycle-corr", "premise");
        let record = fabric_record(&outcomes[0]).unwrap();
        let event = record.decode_payload(PayloadCodec::CanonicalJson).unwrap();
        let chain = outcomes[0]
            .chain_correlation()
            .expect("a fill names its order");
        assert_eq!(event.lineage.correlation_id, chain);
        assert_ne!(event.lineage.correlation_id.as_str(), "cycle-corr");
        assert_eq!(event.topic, REFLEX_OUTCOME_RECORDED.topic);
        assert_eq!(event.event_id.as_str(), event_id(0));
    }

    #[test]
    fn the_event_id_is_a_function_of_the_chain_position_alone() {
        assert_ne!(event_id(0), event_id(1));
        assert_eq!(event_id(7), event_id(7));
        // A rebuild yields the same bytes, which is what makes a resend a
        // duplicate to the ledger rather than a second fact.
        let outcomes = outcome(&["o1", "o2"]);
        assert_eq!(
            fabric_record(&outcomes[1]).unwrap(),
            fabric_record(&outcomes[1]).unwrap()
        );
    }

    #[test]
    fn a_non_simulated_record_is_never_built() {
        // The failure this prevents: a live fill shipped to a ledger that
        // parks its chain on it. The paper boundary holds at this seam too.
        let mut outcomes = outcome(&["o1"]);
        if let Decision::Filled { simulated, .. } = &mut outcomes[0].record.entry.decision {
            *simulated = false;
        }
        let error = fabric_record(&outcomes[0]).unwrap_err();
        assert!(
            error.message().contains("not marked simulated"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn a_record_for_another_chain_or_a_torn_position_is_refused() {
        let mut outcomes = outcome(&["o1"]);
        outcomes[0].record.cell = "cell-x".to_string();
        assert!(fabric_record(&outcomes[0]).is_err());
        let mut outcomes = outcome(&["o1"]);
        outcomes[0].record.journal_sequence = 9;
        assert!(fabric_record(&outcomes[0]).is_err());
    }
}
