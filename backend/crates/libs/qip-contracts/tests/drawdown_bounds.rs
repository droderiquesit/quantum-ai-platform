//! ADR 0126 decision 2 (R12-CAPITAL-4088): the signed envelope carries a
//! drawdown stop, inside the signed payload, refused when it is not positive
//! or wider than the loss limit, and absent on an envelope issued before it
//! existed.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::{CapitalEnvelope, DrawdownBounds};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::{Decimal, Timestamp};

fn d(text: &str) -> Decimal {
    Decimal::parse(text).unwrap()
}

fn envelope(loss_limit: &str) -> CapitalEnvelope {
    CapitalEnvelope::new(
        StrategyId::new("alpha"),
        "london-1",
        d("1000"),
        d("100"),
        d(loss_limit),
        vec![VenueId::new("XLON")],
        Timestamp::from_secs(1_760_000_000),
        Timestamp::from_secs(1_760_003_600),
        "alice@example.com",
        "signed-before",
    )
    .unwrap()
}

/// The stop is a bound only if it can fire before the loss limit does, so
/// one wider is refused at issue; one at the limit is admitted, enters the
/// signed payload with its schema version, and clears the old signature.
///
/// Mutation: drop `drawdown_within_loss_limit` from `with_drawdown` — the
/// wide stop is issued.
#[test]
fn a_drawdown_stop_is_issued_only_within_the_loss_limit_and_is_signed_over() {
    let wide = envelope("100").with_drawdown(DrawdownBounds::bounded(d("150")).unwrap());
    let refused = wide.expect_err("a 150 stop on a 100 loss limit was issued");
    assert!(
        refused.message().contains("looser than the 100 loss limit"),
        "{}",
        refused.message()
    );

    let plain = envelope("100");
    let stopped = envelope("100")
        .with_drawdown(DrawdownBounds::bounded(d("100")).unwrap())
        .expect("a stop at the loss limit is a bound");
    assert_eq!(
        stopped.drawdown().map(|stop| stop.max_drawdown),
        Some(d("100"))
    );
    assert!(
        stopped.signature().is_empty(),
        "the old signature survived the new bound"
    );
    assert!(
        stopped.signing_payload().ends_with("|drawdown.v1|100"),
        "{}",
        stopped.signing_payload()
    );
    assert!(
        !plain.signing_payload().contains("drawdown"),
        "an envelope without a stop signs as it always did: {}",
        plain.signing_payload()
    );
}

/// Not positive is refused at construction and at decode, so a stop of zero
/// cannot arrive on the wire either.
///
/// Mutation: drop the `positive` check from `DrawdownBoundsFields::check`.
#[test]
fn a_non_positive_drawdown_stop_is_refused_at_construction_and_at_decode() {
    assert!(DrawdownBounds::bounded(Decimal::ZERO).is_err());
    assert!(DrawdownBounds::bounded(d("-1")).is_err());
    let wire = serde_json::json!({ "schema_version": 1, "max_drawdown": "0" });
    assert!(
        serde_json::from_value::<DrawdownBounds>(wire).is_err(),
        "a zero stop decoded"
    );
    let good = DrawdownBounds::bounded(d("5")).unwrap();
    let round = serde_json::from_value::<DrawdownBounds>(serde_json::to_value(&good).unwrap())
        .expect("a valid stop round-trips");
    assert_eq!(round, good);
}
