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
use qip_contracts::intent::Contributor;
use qip_contracts::message::BookSide;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_contracts::wire::CrossRecord;
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
    /// Every strategy whose intent went into this order. `strategy` above is
    /// the largest contributor; attributing a netted fill to it alone credits
    /// one strategy with another's trade.
    ///
    /// Defaulted for the same reason the edge defaults it: a delta written
    /// before the field existed still replays out of the sealed log, reading
    /// as having named no contributors, which is what it did.
    #[serde(default)]
    pub contributors: Vec<Contributor>,
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
    #[serde(default)]
    crosses: Vec<CrossRecord>,
    #[serde(default)]
    crosses_omitted: u32,
}

impl EventBody for WireDelta {
    /// The topic the edge crate publishes under; see [`CELL_DELTA_TOPIC`] for
    /// why the two ends agree on a constant rather than a shared declaration.
    const TOPIC: Topic = CELL_DELTA_TOPIC;
    /// Declared once in `qip-contracts` and read by both ends, so the centre
    /// cannot fall behind a cell by a number somebody forgot to change in two
    /// places. It previously said it was "held equal to the edge crate's by the
    /// round-trip tests"; nothing compared the two, and this type is private,
    /// so nothing could.
    ///
    /// Decoding through [`AnyEvent::decode`] means a payload written by a
    /// *newer* schema is refused rather than partially understood — and the
    /// fields a partial read would drop are the ones a newer cell added because
    /// the centre needed them.
    const SCHEMA_VERSION: u32 = qip_contracts::wire::CELL_DELTA_SCHEMA_VERSION;

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
    /// Every internal cross the cell booked in this interval (§27.1) — a trade
    /// between two of the platform's own strategies. Shares its declaration
    /// with the edge crate rather than mirroring it, so this half cannot drift
    /// from the half that writes it.
    pub crosses: Vec<CrossRecord>,
    /// Crosses that did not fit the wire bound, counted for the same reason.
    pub crosses_omitted: u32,
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
            crosses: wire.crosses,
            crosses_omitted: wire.crosses_omitted,
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
    fn the_contributors_behind_a_netted_order_survive_the_decode_intact() -> Result<()> {
        // The centre attributes fills. Since the cell nets, one order can carry
        // several strategies' shares, and `strategy` names only the largest —
        // so a decode that dropped the contributor vector would credit one
        // strategy with another's trade and nothing downstream could tell.
        let mut payload = wire_payload();
        payload["orders"][0]["contributors"] = serde_json::json!([
            {
                "strategy": "mean-reversion-1",
                "signed_size": "60",
                "inputs": [["book_pressure{levels=5}", 11]]
            },
            {
                "strategy": "momentum-2",
                "signed_size": "40",
                "inputs": [["momentum{}", 9]]
            }
        ]);

        let decoded = decode_cell_delta(&frame_with(payload)?)?;
        let order = &decoded.interval.orders[0];
        assert_eq!(
            order.contributors.len(),
            2,
            "the contributor vector did not cross the wire"
        );
        // Signed sizes, not absolute: they must sum to the net rather than to
        // the gross, and a decode that took the magnitude would break that.
        assert_eq!(
            order.contributors[0].signed_size,
            Decimal::parse("60").expect("a decimal literal")
        );
        assert_eq!(order.contributors[1].strategy.as_str(), "momentum-2");
        // Each keeps its own revisions. The union, or one copied onto both,
        // would leave the centre unable to say which values produced which
        // share.
        assert_eq!(
            order.contributors[0].inputs,
            vec![("book_pressure{levels=5}".to_string(), 11)]
        );
        assert_eq!(
            order.contributors[1].inputs,
            vec![("momentum{}".to_string(), 9)]
        );
        assert_ne!(order.contributors[0].inputs, order.contributors[1].inputs);
        Ok(())
    }

    #[test]
    fn the_two_ends_of_the_uplink_read_the_same_schema_version() {
        // The edge crate cannot be named from a service, so the agreement is
        // made of constants. This one used to be written twice under a comment
        // claiming the round-trip tests held the pair equal; nothing compared
        // them, and this type is private, so nothing could. Declaring it once
        // in the crate both ends already depend on is what makes the drift
        // unreachable — and the assertion is that this end still reads it from
        // there rather than having quietly reacquired a literal of its own.
        assert_eq!(
            WireDelta::SCHEMA_VERSION,
            qip_contracts::wire::CELL_DELTA_SCHEMA_VERSION,
            "the centre's delta schema version is no longer the shared one, so \
             it can drift from the cell's"
        );
    }

    #[test]
    fn an_order_from_a_cell_that_predates_contributors_decodes_as_naming_none() -> Result<()> {
        // The event log is sealed and hash-chained. A record written before
        // the field existed has to replay, and it did name no contributors —
        // so an empty vector is the true reading rather than a convenient one.
        // The premise is that the fixture genuinely lacks the field; without
        // it this would assert the default over a payload that set it empty.
        let payload = wire_payload();
        assert!(
            payload["orders"][0].get("contributors").is_none(),
            "the fixture already carries contributors, so this proves nothing"
        );

        let decoded = decode_cell_delta(&frame_with(payload)?)?;
        assert!(decoded.interval.orders[0].contributors.is_empty());
        // And the rest of the order still arrived, so the default did not come
        // from the whole decode quietly failing.
        assert_eq!(decoded.interval.orders[0].order_id, "ord-1");
        assert!(decoded.interval.orders[0].simulated);
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
