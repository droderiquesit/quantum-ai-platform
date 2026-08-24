//! Decoding a cell's state delta, and keeping its two arithmetics apart.
//!
//! [`crate::spine`] deliberately does not decode the frames it absorbs: the
//! delta's declaring type lives in `qip-edge`, an edge crate no service may
//! name, and the sink seam exists so the composition root can supply the
//! decode. For a long time no composition root did — the decode ran only
//! inside an acceptance test — and this module is where it now lives so that
//! every serving binary uses one decode instead of each carrying its own.
//!
//! # Two declarations of one wire shape, on purpose
//!
//! [`WireDelta`] mirrors `qip_edge::mesh::CellStateDelta` field for field.
//! They are written twice because the dependency edge between an edge crate
//! and a service runs one way only, exactly as [`crate::spine`]'s grant key
//! mirrors the cell's: the compiler cannot hold them together, so the
//! round-trip tests do — this crate's against the recorded wire shape, and
//! the composition root's against a delta built by the edge crate itself. A
//! field added on one side and not the other is caught there, not here.
//!
//! # The two halves have different arithmetic, and the types say so
//!
//! A delta's *standing* fields — utilisation, the halt flag, the
//! reconciliation breaks — are **absolute**: they describe the cell as it
//! stands, and a centre that summed them across deltas would drift from the
//! cell it is describing at exactly the moment a message was lost. Its
//! *interval* fields — the orders and refusals since the previous delta —
//! are **incremental**: they cover one interval, and a centre that
//! overwrote them would lose the activity of every interval it did not
//! sample. The decode returns them as two separate types rather than one
//! struct with a comment, because the comment is the thing a later caller
//! does not read: a [`CellStanding`] replaces what the centre holds, a
//! [`CellInterval`] adds to it, and a caller cannot take one for the other
//! without naming the wrong type.

use qip_contracts::capital::Utilisation;
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_events::{AnyEvent, EventBody, Topic};
use serde::{Deserialize, Serialize};

use crate::spine::CELL_DELTA_TOPIC;

/// One order the cell sent during the interval a delta covers.
///
/// Mirrors `qip_edge::mesh::DeltaOrder`. The `simulated` flag crosses the
/// wire untouched because a paper fill counted as real is the single most
/// consequential bit in the execution path, and a decode that defaulted it
/// would be the place that flipped it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeltaOrder {
    pub order_id: String,
    pub strategy: StrategyId,
    pub object_id: ObjectId,
    pub venue: VenueId,
    pub side: BookSide,
    pub quantity: Decimal,
    pub price: Decimal,
    pub simulated: bool,
}

/// What one strategy has committed against its envelope, absolute.
///
/// Mirrors `qip_edge::mesh::StrategyUtilisation`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StrategyStanding {
    pub strategy: StrategyId,
    pub utilisation: Utilisation,
    /// When the grant this utilisation is measured against runs out. The
    /// centre reads it to see which cells are about to stop trading on their
    /// own.
    pub envelope_expires_at: Timestamp,
}

/// A gate that refused during the interval, and what it said.
///
/// Mirrors `qip_edge::mesh::DeltaRefusal`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeltaRefusal {
    pub gate: String,
    pub reason: String,
}

/// The wire shape of a cell's state delta, private to the decode.
///
/// Field names and the serde defaults must match
/// `qip_edge::mesh::CellStateDelta` exactly — this struct exists to be
/// deserialised from the payload that type serialised. It is not public:
/// callers get the two typed halves, so the flat wire shape cannot leak into
/// code that would then accumulate an absolute field because nothing stopped
/// it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct WireDelta {
    cell: String,
    region: String,
    sequence: u64,
    at: Timestamp,
    halted: bool,
    utilisation: Vec<StrategyStanding>,
    orders: Vec<DeltaOrder>,
    refusals: Vec<DeltaRefusal>,
    #[serde(default)]
    refusals_omitted: u32,
    reconciliation_breaks: Vec<String>,
    #[serde(default)]
    reconciliation_breaks_omitted: u32,
}

impl EventBody for WireDelta {
    /// The topic the edge crate publishes under; see [`CELL_DELTA_TOPIC`] for
    /// why the two ends agree on a constant rather than a shared declaration.
    const TOPIC: Topic = CELL_DELTA_TOPIC;
    /// Held equal to the edge crate's by the round-trip tests. Decoding
    /// through [`AnyEvent::decode`] means a payload written by a *newer*
    /// schema is refused rather than partially understood — and the fields a
    /// partial read would drop are the ones a newer cell added because the
    /// centre needed them.
    const SCHEMA_VERSION: u32 = 1;

    /// Cell and sequence — the same key the cell stamps, so identity survives
    /// the decode.
    fn idempotency_key(&self) -> Option<String> {
        Some(format!("{}:{}", self.cell, self.sequence))
    }
}

/// The absolute half: the cell as it stands.
///
/// A receiver **replaces** what it holds for this cell with these fields.
/// Summing them across deltas is the drift the module documentation
/// describes, and it begins exactly when a message is lost.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellStanding {
    pub cell: String,
    pub region: String,
    /// The cell's own monotonic counter — the delta's identity, carried so a
    /// caller can say *which* statement of the cell's standing this is.
    pub sequence: u64,
    pub at: Timestamp,
    /// Whether the cell has stopped itself. First field an incident reader
    /// looks at.
    pub halted: bool,
    pub utilisation: Vec<StrategyStanding>,
    /// Every disagreement between the cell's fills and the venue's own
    /// account, as the cell described them. Prose rather than a structured
    /// break, because prose is what the cell shipped; a decode that parsed
    /// quantities out of it would be inventing numbers nobody sent.
    pub reconciliation_breaks: Vec<String>,
    /// Breaks the cell recorded but no longer retains. Non-zero means the
    /// list above understates the incident.
    pub reconciliation_breaks_omitted: u32,
}

/// The incremental half: what happened since the previous delta.
///
/// A receiver **adds** these to what it holds. Overwriting them would lose
/// the orders and refusals of every interval it did not sample.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CellInterval {
    pub orders: Vec<DeltaOrder>,
    pub refusals: Vec<DeltaRefusal>,
    /// Refusals that did not fit the wire bound. Counted so a truncation is
    /// visible; an accumulator adds this too or it undercounts.
    pub refusals_omitted: u32,
}

/// One decoded delta, halved by arithmetic.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedCellDelta {
    pub standing: CellStanding,
    pub interval: CellInterval,
}

/// Decode one absorbed frame into the two halves.
///
/// Pure: reads the frame, writes nothing, and every refusal is
/// [`AnyEvent::decode`]'s — a topic that is not a cell delta, a schema newer
/// than this build, or a payload that does not parse. The payload *hash* is
/// deliberately not re-checked here because the callers this function exists
/// for receive frames from [`crate::spine::CellDeltaReceiver`], which has
/// already refused a frame whose payload no longer matches its hash; a
/// second check would imply this one could disagree with the first.
pub fn decode_cell_delta(frame: &AnyEvent) -> Result<DecodedCellDelta> {
    let wire = frame.decode::<WireDelta>()?.body;
    if wire.cell.trim().is_empty() {
        // Refused here rather than passed through, because everything a
        // receiver does with a delta — replace this cell's standing, key its
        // idempotency — is keyed by the cell's name, and an empty key would
        // silently merge every anonymous cell into one.
        return Err(Error::invalid(
            "the delta names no cell, so there is nothing to file its standing under",
        ));
    }
    Ok(DecodedCellDelta {
        standing: CellStanding {
            cell: wire.cell,
            region: wire.region,
            sequence: wire.sequence,
            at: wire.at,
            halted: wire.halted,
            utilisation: wire.utilisation,
            reconciliation_breaks: wire.reconciliation_breaks,
            reconciliation_breaks_omitted: wire.reconciliation_breaks_omitted,
        },
        interval: CellInterval {
            orders: wire.orders,
            refusals: wire.refusals,
            refusals_omitted: wire.refusals_omitted,
        },
    })
}

#[cfg(test)]
mod tests {
    // In a test the assertion is the deliverable; the workspace denies this
    // lint for production code, where a panic on this path would be a bug.
    #![allow(clippy::panic_in_result_fn)]

    use super::*;
    use qip_core::{CorrelationId, Id, Lineage};
    use qip_events::Envelope;

    /// The wire payload as the edge crate writes it, spelled out by hand.
    ///
    /// Deliberately JSON text rather than a struct this module serialised:
    /// a mirror tested against itself proves only that serde round-trips,
    /// while this pins the exact field names the two crates must agree on.
    /// The cross-crate half of the proof — a delta built by `qip-edge`
    /// itself decoding through this function — lives with the composition
    /// root, which is the one place allowed to name both ends.
    fn wire_payload() -> serde_json::Value {
        serde_json::json!({
            "cell": "london-1",
            "region": "eu-west",
            "sequence": 7,
            "at": Timestamp::from_secs(1_000),
            "halted": true,
            "utilisation": [{
                "strategy": "mean-reversion-1",
                "utilisation": {
                    "gross_committed": "250000",
                    "realised_loss": "1200",
                    "orders_sent": 14
                },
                "envelope_expires_at": Timestamp::from_secs(5_000)
            }],
            "orders": [{
                "order_id": "ord-1",
                "strategy": "mean-reversion-1",
                "object_id": "OBJEQUITY1",
                "venue": "XLON",
                "side": "Bid",
                "quantity": "100",
                "price": "99.5",
                "simulated": true
            }],
            "refusals": [{"gate": "pre-trade-risk", "reason": "gross limit"}],
            "refusals_omitted": 3,
            "reconciliation_breaks": ["OBJEQUITY1: cell holds 100, venue confirms 60"],
            "reconciliation_breaks_omitted": 1
        })
    }

    fn frame_with(payload: serde_json::Value) -> Result<AnyEvent> {
        let wire: WireDelta =
            serde_json::from_value(payload).map_err(|error| Error::schema(error.to_string()))?;
        Envelope::new(
            Id::from_string("EVTCELLTEST00000000000000001"),
            Timestamp::from_secs(1_000),
            Timestamp::from_secs(1_001),
            Lineage::root(CorrelationId::from_string("CORCELLTEST1"), "test"),
            wire,
        )
        .erase()
    }

    #[test]
    fn the_absolute_fields_land_in_the_standing_and_the_incremental_ones_in_the_interval()
    -> Result<()> {
        // The split is the whole point of the type: a caller holding a
        // CellStanding replaces, a caller holding a CellInterval accumulates,
        // and this test pins which field went to which side.
        let decoded = decode_cell_delta(&frame_with(wire_payload())?)?;

        assert_eq!(decoded.standing.cell, "london-1");
        assert_eq!(decoded.standing.sequence, 7);
        assert!(decoded.standing.halted);
        assert_eq!(decoded.standing.utilisation.len(), 1);
        assert_eq!(
            decoded.standing.utilisation[0].utilisation.orders_sent, 14,
            "the utilisation that crossed is not the one the cell stated"
        );
        assert_eq!(decoded.standing.reconciliation_breaks.len(), 1);
        assert_eq!(
            decoded.standing.reconciliation_breaks_omitted, 1,
            "an understated break list must say it understates"
        );

        assert_eq!(decoded.interval.orders.len(), 1);
        assert!(
            decoded.interval.orders[0].simulated,
            "the paper flag was not carried across the decode"
        );
        assert_eq!(decoded.interval.refusals.len(), 1);
        assert_eq!(decoded.interval.refusals_omitted, 3);
        Ok(())
    }

    #[test]
    fn a_frame_on_another_topic_is_refused_rather_than_misread() -> Result<()> {
        // Reuse a real frame and change only the topic, so the refusal below
        // can only be about the topic and not about the payload.
        let mut frame = frame_with(wire_payload())?;
        frame.topic = Topic::ServiceStarted;
        let error = decode_cell_delta(&frame)
            .expect_err("a service-lifecycle frame decoded as a cell delta");
        assert!(
            error.message().contains(Topic::ServiceStarted.name()),
            "the refusal does not name the topic it refused: {}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn a_delta_written_by_a_newer_schema_is_refused_rather_than_partially_understood() -> Result<()>
    {
        let mut frame = frame_with(wire_payload())?;
        frame.schema_version = WireDelta::SCHEMA_VERSION + 1;
        let error = decode_cell_delta(&frame)
            .expect_err("a newer-schema delta was decoded with fields silently dropped");
        assert!(
            error.message().contains("newer"),
            "the refusal does not say the schema is newer: {}",
            error.message()
        );
        Ok(())
    }

    #[test]
    fn a_delta_naming_no_cell_is_refused_because_standing_needs_a_key() -> Result<()> {
        let mut payload = wire_payload();
        payload["cell"] = serde_json::json!("  ");
        let error =
            decode_cell_delta(&frame_with(payload)?).expect_err("an anonymous delta was accepted");
        assert!(error.message().contains("names no cell"));
        Ok(())
    }

    #[test]
    fn the_omission_counters_default_to_zero_for_a_sender_that_predates_them() -> Result<()> {
        // The edge type marks both counters `#[serde(default)]`; the mirror
        // must accept the same older payload or the two ends have drifted.
        let mut payload = wire_payload();
        let Some(object) = payload.as_object_mut() else {
            return Err(Error::invalid("the fixture payload is not an object"));
        };
        object.remove("refusals_omitted");
        object.remove("reconciliation_breaks_omitted");
        let decoded = decode_cell_delta(&frame_with(payload)?)?;
        assert_eq!(decoded.interval.refusals_omitted, 0);
        assert_eq!(decoded.standing.reconciliation_breaks_omitted, 0);
        Ok(())
    }

    #[test]
    fn the_decode_keys_identity_on_cell_and_sequence_like_the_edge_does() -> Result<()> {
        // The receiver's idempotency is keyed on the frame's dedup key,
        // which is the topic plus the body's own idempotency key. If the
        // mirror stamped a different key shape than the edge crate, a
        // redelivery would stop being recognisable the moment it was decoded
        // through this module.
        let frame = frame_with(wire_payload())?;
        assert_eq!(
            frame.dedup_key(),
            format!("{}:london-1:7", CELL_DELTA_TOPIC.name())
        );
        Ok(())
    }
}
