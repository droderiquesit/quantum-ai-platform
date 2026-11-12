//! The typed cross-asset and cross-region sync record (R12-TICK-2048,
//! ADR 0438). Before it, the lake line's sync stamp was untyped JSON nobody
//! decoded, so a line claiming an order its own lag could not support was a
//! line a lead/lag model would learn from. These tests hold the refusals a
//! decoder must re-run.
#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::sync::{
    AssetKey, ClockSource, CrossPartition, MAX_SYNC_ALIGNMENTS, PartitionAlignment, SyncMetadata,
    SyncOrder,
};

fn asset() -> AssetKey {
    AssetKey {
        venue: "XLON".to_string(),
        feed: "depth".to_string(),
        partition: 0,
        instrument: "ACME".to_string(),
    }
}

fn other(lag_ns: i64, lag_uncertainty_ns: i64, order: SyncOrder) -> PartitionAlignment {
    PartitionAlignment {
        partition: "XPAR/depth/0/ACME".to_string(),
        venue: "XPAR".to_string(),
        instrument: "ACME".to_string(),
        sequence: 7,
        order_of_other: order,
        lag_ns,
        lag_uncertainty_ns,
        other_known_at_arrival: true,
    }
}

fn aligned(alignments: Vec<PartitionAlignment>) -> SyncMetadata {
    SyncMetadata::new(
        "europe-west2",
        "edge-1",
        ClockSource::OffsetEstimate,
        asset(),
        CrossPartition::Aligned(alignments),
    )
    .expect("premise: a well-formed record is admitted")
}

/// Every field a cross-region or cross-asset reader needs survives the wire,
/// and the wire keys are the ones the lake line holds at its top level.
#[test]
fn a_sync_record_round_trips_with_its_region_cell_clock_and_asset_key() {
    let sync = aligned(vec![other(3_000, 2_000, SyncOrder::Before)]);
    let value = serde_json::to_value(&sync).unwrap();
    for key in [
        "region",
        "cell",
        "clock_source",
        "venue",
        "feed",
        "partition",
        "instrument",
        "cross_partition",
    ] {
        assert!(value.get(key).is_some(), "wire form lost `{key}`: {value}");
    }
    assert_eq!(value["region"], "europe-west2");
    assert_eq!(value["instrument"], "ACME");
    assert_eq!(value["cross_partition"][0]["venue"], "XPAR");
    let back: SyncMetadata = serde_json::from_value(value).unwrap();
    assert_eq!(back, sync);
    assert_eq!(back.region(), "europe-west2");
    assert_eq!(back.asset(), asset());
}

/// The forged order: a line saying the other tick came `before` when the lag
/// is inside both clocks' error. Refused at construction and at decode.
#[test]
fn an_order_the_lag_and_its_uncertainty_do_not_support_is_refused_on_decode() {
    let honest = aligned(vec![other(2_000, 2_000, SyncOrder::Unordered)]);
    let mut value = serde_json::to_value(&honest).unwrap();
    assert!(serde_json::from_value::<SyncMetadata>(value.clone()).is_ok());
    value["cross_partition"][0]["order_of_other"] = serde_json::json!("before");
    let error = serde_json::from_value::<SyncMetadata>(value).unwrap_err();
    assert!(
        error.to_string().contains("which supports Unordered"),
        "refused for another reason: {error}"
    );
    // And the reverse sign: a lag beyond the bound the other way is `after`.
    assert!(
        SyncMetadata::new(
            "europe-west2",
            "edge-1",
            ClockSource::OffsetEstimate,
            asset(),
            CrossPartition::Aligned(vec![other(-3_000, 2_000, SyncOrder::Before)]),
        )
        .is_err()
    );
}

/// A line that does not say which region stamped it cannot be placed against
/// another region's clock; nor can one whose asset key is blank be learned
/// across instruments.
#[test]
fn a_record_without_a_region_cell_or_asset_key_is_refused() {
    let good = serde_json::to_value(aligned(Vec::new())).unwrap();
    for (key, blank) in [
        ("region", ""),
        ("region", "europe west2"),
        ("cell", ""),
        ("venue", " "),
        ("feed", ""),
        ("instrument", ""),
    ] {
        let mut value = good.clone();
        value[key] = serde_json::json!(blank);
        assert!(
            serde_json::from_value::<SyncMetadata>(value).is_err(),
            "a record with {key}={blank:?} was admitted"
        );
    }
    for key in ["region", "cell", "instrument", "venue"] {
        let mut value = good.clone();
        value.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<SyncMetadata>(value).is_err(),
            "a record missing `{key}` was admitted"
        );
    }
}

/// The clock source and the alignment say the same fact and must agree.
#[test]
fn an_unplaced_tick_with_alignments_and_a_placed_tick_with_a_refusal_are_refused() {
    let refused = CrossPartition::Refused {
        refused: "discipline refused".to_string(),
    };
    assert!(SyncMetadata::new("r", "c", ClockSource::Unplaced, asset(), refused.clone()).is_ok());
    assert!(
        SyncMetadata::new("r", "c", ClockSource::VenueStated, asset(), refused.clone()).is_err()
    );
    assert!(
        SyncMetadata::new(
            "r",
            "c",
            ClockSource::Unplaced,
            asset(),
            CrossPartition::Aligned(Vec::new())
        )
        .is_err()
    );
}

/// An alignment against the tick's own partition, more than the capture
/// path's bound, or with a negative uncertainty, was not written by it.
#[test]
fn an_alignment_against_itself_past_the_bound_or_with_negative_doubt_is_refused() {
    let mut own = other(5_000, 1_000, SyncOrder::Before);
    own.partition = asset().path();
    let negative = other(5_000, -1, SyncOrder::Before);
    let too_many: Vec<PartitionAlignment> = (0..=MAX_SYNC_ALIGNMENTS)
        .map(|index| {
            let mut alignment = other(0, 1, SyncOrder::Unordered);
            alignment.partition = format!("XPAR/depth/{index}/ACME");
            alignment
        })
        .collect();
    assert_eq!(too_many.len(), MAX_SYNC_ALIGNMENTS + 1, "premise");
    let at_bound: Vec<PartitionAlignment> = too_many[..MAX_SYNC_ALIGNMENTS].to_vec();
    let build = |alignments| {
        SyncMetadata::new(
            "r",
            "c",
            ClockSource::OffsetEstimate,
            asset(),
            CrossPartition::Aligned(alignments),
        )
    };
    assert!(
        build(at_bound).is_ok(),
        "premise: the bound itself is admitted"
    );
    assert!(build(vec![own]).is_err(), "own partition admitted");
    assert!(build(vec![negative]).is_err(), "negative doubt admitted");
    assert!(build(too_many).is_err(), "past the bound admitted");
}
