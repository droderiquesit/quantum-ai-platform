//! R12-GOV-6120: a capital envelope's signature covers leverage, product
//! classes and regulated functions, and the bounded admission refuses
//! outside them.
//!
//! Each test names the mutation that should make it fail.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use qip_contracts::capital::{AuthorityBounds, CapitalAdmission, CapitalEnvelope, Utilisation};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::{Decimal, Timestamp};

fn d(n: i64) -> Decimal {
    Decimal::from_int(n)
}

fn envelope() -> CapitalEnvelope {
    CapitalEnvelope::new(
        StrategyId::new("momentum"),
        "cell-us",
        d(1_000),
        d(100),
        d(50),
        vec![VenueId::new("venue-a")],
        Timestamp::from_secs(0),
        Timestamp::from_secs(1_000),
        "risk-officer",
        "sig",
    )
    .unwrap()
}

fn bounds(leverage: i64, classes: &[&str], functions: &[&str]) -> AuthorityBounds {
    AuthorityBounds::new(
        d(leverage),
        classes.iter().map(|c| c.to_string()).collect(),
        functions.iter().map(|f| f.to_string()).collect(),
    )
    .unwrap()
}

/// Every bound moves the signed payload, so a signature over one set of
/// bounds does not verify over another. Mutation: leave any one bound out
/// of `signing_payload` — its row's two payloads are then equal.
#[test]
fn the_signed_payload_covers_leverage_product_classes_and_regulated_functions() {
    let unbounded = envelope();
    let base = envelope()
        .bound(bounds(2, &["equity"], &["market_making"]))
        .unwrap();
    assert_ne!(
        base.signing_payload(),
        unbounded.signing_payload(),
        "premise"
    );
    assert!(
        base.signature().is_empty(),
        "bounding clears the old signature"
    );
    let variants = [
        ("leverage", bounds(3, &["equity"], &["market_making"])),
        (
            "product class",
            bounds(2, &["equity", "futures"], &["market_making"]),
        ),
        ("regulated function", bounds(2, &["equity"], &[])),
    ];
    for (bound, other) in variants {
        let changed = envelope().bound(other).unwrap();
        assert_ne!(changed.signing_payload(), base.signing_payload(), "{bound}");
    }
    // Order does not matter; the set does.
    let reordered = envelope()
        .bound(bounds(2, &["futures", "equity"], &["market_making"]))
        .unwrap();
    let sorted = envelope()
        .bound(bounds(2, &["equity", "futures"], &["market_making"]))
        .unwrap();
    assert_eq!(reordered.signing_payload(), sorted.signing_payload());
    // Widening is a new envelope, not a second bound on this one.
    assert!(base.bound(bounds(9, &["equity"], &[])).is_err());
}

/// Mutation: drop the product-class or leverage branch of `admit_bounded`.
#[test]
fn a_bounded_admission_refuses_outside_the_class_the_leverage_or_the_function() {
    let used = Utilisation::default();
    let venue = VenueId::new("venue-a");
    let now = Timestamp::from_secs(10);
    let bounded = envelope()
        .bound(bounds(2, &["equity"], &["market_making"]))
        .unwrap()
        .signed("re-signed");
    assert_eq!(
        bounded.admit_bounded(
            &venue,
            d(10),
            &used,
            now,
            "equity",
            d(2),
            Some("market_making")
        ),
        CapitalAdmission::Full,
        "premise"
    );
    for (why, admission) in [
        (
            "class",
            bounded.admit_bounded(&venue, d(10), &used, now, "futures", d(1), None),
        ),
        (
            "leverage",
            bounded.admit_bounded(&venue, d(10), &used, now, "equity", d(3), None),
        ),
        (
            "function",
            bounded.admit_bounded(&venue, d(10), &used, now, "equity", d(1), Some("custody")),
        ),
        (
            "unbounded envelope",
            envelope().admit_bounded(&venue, d(10), &used, now, "equity", d(1), None),
        ),
    ] {
        assert!(admission.is_refused(), "{why}: {admission:?}");
    }
    assert!(
        AuthorityBounds::new(
            Decimal::from_scaled(5, 1).unwrap(),
            vec!["equity".into()],
            vec![]
        )
        .is_err()
    );
    assert!(AuthorityBounds::new(d(1), vec![], vec![]).is_err());
}

/// ADR 0155 section 4: there is one leverage measure, so an envelope that
/// carries both blocks must state one claim about it, in either order of
/// construction. Mutation: drop the equality check in `bound` or
/// `with_exposure` and the matching arm below accepts a contradiction.
#[test]
fn an_envelope_stating_two_different_leverages_is_refused_whichever_block_is_added_second() {
    use qip_contracts::capital::ExposureBounds;
    let exposure = |leverage: i64| ExposureBounds::bounded(2_500, d(leverage), d(1_000)).unwrap();

    // Premise: equal claims build, in both orders.
    assert!(
        envelope()
            .with_exposure(exposure(2))
            .unwrap()
            .bound(bounds(2, &["equity"], &["order_taking"]))
            .is_ok()
    );
    assert!(
        envelope()
            .bound(bounds(2, &["equity"], &["order_taking"]))
            .unwrap()
            .with_exposure(exposure(2))
            .is_ok()
    );

    let refused = envelope()
        .with_exposure(exposure(2))
        .unwrap()
        .bound(bounds(3, &["equity"], &["order_taking"]))
        .unwrap_err()
        .to_string();
    assert!(refused.contains("one leverage measure"), "{refused}");
    let refused = envelope()
        .bound(bounds(3, &["equity"], &["order_taking"]))
        .unwrap()
        .with_exposure(exposure(2))
        .unwrap_err()
        .to_string();
    assert!(refused.contains("one leverage measure"), "{refused}");
}
