//! ADR 0126 (R12-CONTRACT-7066, R12-CAPITAL-4088): the signed envelope — the
//! one downward capital authority — carries the drawdown stop, liquidity
//! reserve and ADV share the `CapitalGrant` used to restate, inside its
//! signed payload, refused rather than clamped, and round-tripped whole.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::DrawdownBounds;
use qip_contracts::capital::CapitalEnvelope;
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::{Decimal, Timestamp};

fn d(text: &str) -> Decimal {
    Decimal::parse(text).unwrap()
}

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

/// 3,000 gross, 1,500 per order, 500 loss.
fn plain() -> CapitalEnvelope {
    CapitalEnvelope::new(
        StrategyId::new("alpha"),
        "london-1",
        d("3000"),
        d("1500"),
        d("500"),
        vec![VenueId::new("XLON")],
        at(0),
        at(3600),
        "alice@example.com",
        "",
    )
    .unwrap()
}

fn block(stop: &str) -> DrawdownBounds {
    DrawdownBounds::bounded(d(stop), Some(d("250")), Some(1_000)).unwrap()
}

/// Mutation: drop the drawdown segment from `signing_payload` and the two
/// stops sign the same bytes, so this fails.
#[test]
fn the_drawdown_block_is_inside_the_signed_payload_and_each_figure_changes_it() {
    let tight = plain().with_drawdown(block("100")).unwrap();
    let loose = plain().with_drawdown(block("400")).unwrap();
    let unstated = plain()
        .with_drawdown(DrawdownBounds::bounded(d("100"), None, None).unwrap())
        .unwrap();
    assert!(
        tight
            .signing_payload()
            .starts_with(&plain().signing_payload()),
        "premise: the block appends to the payload an older envelope signs"
    );
    assert_ne!(tight.signing_payload(), plain().signing_payload());
    assert_ne!(tight.signing_payload(), loose.signing_payload());
    assert_ne!(
        tight.signing_payload(),
        unstated.signing_payload(),
        "an unstated reserve and ADV share sign the same as stated ones"
    );
}

#[test]
fn an_envelope_with_the_block_round_trips_every_field() {
    let envelope = plain().with_drawdown(block("100")).unwrap().signed("sig");
    let wire = serde_json::to_string(&envelope).unwrap();
    let back: CapitalEnvelope = serde_json::from_str(&wire).unwrap();
    assert_eq!(back, envelope);
    let read = back
        .drawdown()
        .expect("the block did not survive the round trip");
    assert_eq!(read.max_drawdown, d("100"));
    assert_eq!(read.liquidity_reserve, Some(d("250")));
    assert_eq!(read.max_adv_share_bp, Some(1_000));
    assert_eq!(back.signing_payload(), envelope.signing_payload());
}

/// Every field of the block is required once it is present, and a widened
/// figure is refused at decode rather than enforced as sent.
#[test]
fn a_block_missing_a_field_or_widened_past_its_range_does_not_decode() {
    let good = serde_json::to_value(block("100")).unwrap();
    assert!(serde_json::from_value::<DrawdownBounds>(good.clone()).is_ok());
    for field in ["schema_version", "max_drawdown"] {
        let mut missing = good.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<DrawdownBounds>(missing).is_err(),
            "a block missing `{field}` decoded"
        );
    }
    let mut unknown = good.clone();
    unknown["max_loss"] = serde_json::json!("1");
    assert!(serde_json::from_value::<DrawdownBounds>(unknown).is_err());
    for (field, value) in [
        ("max_drawdown", serde_json::to_value(d("0")).unwrap()),
        ("liquidity_reserve", serde_json::to_value(d("-1")).unwrap()),
        ("max_adv_share_bp", serde_json::json!(0)),
        ("max_adv_share_bp", serde_json::json!(10_001)),
    ] {
        let mut widened = good.clone();
        widened[field] = value.clone();
        assert!(
            serde_json::from_value::<DrawdownBounds>(widened).is_err(),
            "`{field}` = {value} decoded"
        );
    }
}

/// A stop above the loss limit could never fire first; a reserve above the
/// gross limit holds back more than was granted; a second block is a new
/// envelope. All three are refused, and a stop at the loss limit is not.
#[test]
fn a_stop_looser_than_the_loss_limit_a_reserve_past_the_grant_and_a_second_block_are_refused() {
    assert!(
        plain().with_drawdown(block("500")).is_ok(),
        "premise: the loss limit itself is admitted"
    );
    assert!(plain().with_drawdown(block("500.01")).is_err());
    let over_reserved = DrawdownBounds::bounded(d("100"), Some(d("3000.01")), None).unwrap();
    assert!(plain().with_drawdown(over_reserved).is_err());
    let once = plain().with_drawdown(block("100")).unwrap();
    assert!(once.with_drawdown(block("50")).is_err());
}

#[test]
fn the_stop_refuses_at_the_bound_and_not_before() {
    let envelope = plain().with_drawdown(block("100")).unwrap();
    assert!(envelope.check_drawdown(d("99.99")).is_ok());
    assert!(envelope.check_drawdown(d("100")).is_err());
    assert!(
        plain().check_drawdown(d("1000000")).is_ok(),
        "an envelope signed with no stop is bounded by its loss limit alone"
    );
}
