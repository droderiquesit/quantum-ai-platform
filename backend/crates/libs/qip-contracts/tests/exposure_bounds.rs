//! R12-CAPITAL-4088: the signed envelope carries a liquidity and a leverage
//! bound, both inside the signed payload, both refused rather than clamped,
//! and each one able to refuse an order the money limits admit.
//!
//! Until these bounds the envelope bounded how much a cell could commit and
//! nothing about whether what it built could be exited or was carried on
//! equity the grant still had; the `CapitalGrant` contract that named both
//! was constructed by nothing in production.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_contracts::capital::{
    CapitalAdmission, CapitalEnvelope, ExposureBounds, ExposureBreach, Utilisation,
};
use qip_contracts::signal::StrategyId;
use qip_contracts::venue::VenueId;
use qip_core::{Decimal, Timestamp};

fn d(text: &str) -> Decimal {
    Decimal::parse(text).unwrap()
}

fn at(secs: i64) -> Timestamp {
    Timestamp::from_secs(1_760_000_000 + secs)
}

fn venue() -> VenueId {
    VenueId::new("XLON")
}

/// A grant of 3,000 gross, 1,500 per order, 500 loss, on 1,500 of capital
/// at leverage two — so the gross limit is exactly what the leverage can
/// carry on the whole capital, and the leverage binds only once loss has
/// eaten into it.
fn bounded(participation_bp: u32) -> CapitalEnvelope {
    CapitalEnvelope::new(
        StrategyId::new("alpha"),
        "london-1",
        d("3000"),
        d("1500"),
        d("500"),
        vec![venue()],
        at(0),
        at(3600),
        "alice@example.com",
        "",
    )
    .unwrap()
    .with_exposure(ExposureBounds::bounded(participation_bp, d("2"), d("1500")).unwrap())
    .unwrap()
}

/// Mutation: drop the `exposure` segment from `signing_payload` — the
/// payloads of the two envelopes compare equal and the assertion fails.
#[test]
fn the_bounds_are_inside_the_signed_payload_and_carry_their_schema_version() {
    let unbounded = CapitalEnvelope::new(
        StrategyId::new("alpha"),
        "london-1",
        d("3000"),
        d("1500"),
        d("500"),
        vec![venue()],
        at(0),
        at(3600),
        "alice@example.com",
        "",
    )
    .unwrap();
    let narrow = bounded(100);
    let wide = bounded(2_500);
    assert!(
        narrow
            .signing_payload()
            .starts_with(&unbounded.signing_payload()),
        "premise: the bounds append to the payload an older envelope signs"
    );
    assert_ne!(narrow.signing_payload(), unbounded.signing_payload());
    assert_ne!(narrow.signing_payload(), wide.signing_payload());
    assert!(
        narrow
            .signing_payload()
            .ends_with("|exposure.v1|100|2|1500"),
        "{}",
        narrow.signing_payload()
    );
}

/// A bound widened on the wire is refused at decode, not enforced as sent;
/// a schema version this build does not know is refused rather than read as
/// this one; an envelope written before the bounds still reads.
#[test]
fn a_bound_outside_its_range_or_of_an_unknown_version_does_not_decode() {
    let text = serde_json::to_string(&bounded(2_500)).unwrap();
    let premise: CapitalEnvelope = serde_json::from_str(&text).unwrap();
    assert_eq!(
        premise.exposure().map(|b| b.max_participation_bp),
        Some(2_500)
    );

    for (from, to) in [
        (
            "\"max_participation_bp\":2500",
            "\"max_participation_bp\":0",
        ),
        (
            "\"max_participation_bp\":2500",
            "\"max_participation_bp\":10001",
        ),
        ("\"schema_version\":1", "\"schema_version\":2"),
    ] {
        assert!(text.contains(from), "premise: {from} is in {text}");
        let edited = text.replace(from, to);
        assert!(
            serde_json::from_str::<CapitalEnvelope>(&edited).is_err(),
            "{to} decoded"
        );
    }

    let old = text
        .split(",\"exposure\"")
        .next()
        .map(|head| format!("{head}}}"))
        .unwrap();
    let read: CapitalEnvelope = serde_json::from_str(&old).unwrap();
    assert!(read.exposure().is_none());
}

/// Refused rather than clamped: a participation of zero or past the whole
/// side, leverage below one, no capital, and a gross limit the leverage
/// could never carry on the capital — a limit that cannot fire.
#[test]
fn incoherent_bounds_are_refused_at_construction() {
    assert!(ExposureBounds::bounded(2_500, d("1"), d("1")).is_ok());
    assert!(ExposureBounds::bounded(0, d("1"), d("1")).is_err());
    assert!(ExposureBounds::bounded(10_001, d("1"), d("1")).is_err());
    assert!(ExposureBounds::bounded(2_500, d("0.5"), d("1")).is_err());
    assert!(ExposureBounds::bounded(2_500, d("1"), d("0")).is_err());

    let base = || {
        CapitalEnvelope::new(
            StrategyId::new("alpha"),
            "london-1",
            d("3000"),
            d("1500"),
            d("500"),
            vec![venue()],
            at(0),
            at(3600),
            "alice@example.com",
            "",
        )
        .unwrap()
    };
    let short = ExposureBounds::bounded(2_500, d("1"), d("2999")).unwrap();
    assert!(
        base().with_exposure(short).is_err(),
        "3,000 gross on 2,999 unlevered"
    );
    let enough = ExposureBounds::bounded(2_500, d("1"), d("3000")).unwrap();
    let once = base().with_exposure(enough.clone()).unwrap();
    assert!(
        once.with_exposure(enough).is_err(),
        "a changed bound is a new envelope"
    );
}

/// Mutation: in `check_exposure`, compare against `visible_depth` instead
/// of the participation share — the 26-of-100 order is admitted and the
/// assertion fails.
#[test]
fn an_order_taking_more_of_the_visible_depth_than_the_grant_allows_is_refused() {
    let envelope = bounded(2_500);
    let used = Utilisation::default();
    // Premise: the money limits admit both orders below.
    assert_eq!(
        envelope.admit(&venue(), d("260"), &used, at(10)),
        CapitalAdmission::Full
    );
    assert_eq!(
        envelope.check_exposure(d("25"), d("100"), d("250"), &used),
        Ok(())
    );
    assert!(matches!(
        envelope.check_exposure(d("26"), d("100"), d("260"), &used),
        Err(ExposureBreach::Liquidity(_))
    ));
    assert!(
        matches!(
            envelope.check_exposure(d("1"), Decimal::ZERO, d("10"), &used),
            Err(ExposureBreach::Liquidity(_))
        ),
        "a book showing nothing is not one the cell may assume is deep"
    );
}

/// Mutation: in `check_exposure`, measure leverage on `capital` rather than
/// `capital - realised_loss` — the order after the loss is admitted.
#[test]
fn an_order_carrying_the_grant_past_its_leverage_on_remaining_equity_is_refused() {
    let envelope = bounded(10_000);
    // 1,900 committed, then 1,010 more: 2,910 gross, inside the 3,000 gross
    // limit and inside 2 × 1,500 on the whole capital.
    let unscathed = Utilisation {
        gross_committed: d("1900"),
        realised_loss: Decimal::ZERO,
        orders_sent: 2,
    };
    assert_eq!(
        envelope.check_exposure(d("10"), d("1000"), d("1010"), &unscathed),
        Ok(()),
        "premise: without a loss the same gross is carried"
    );
    // After 120 of realised loss the equity is 1,380 and carries 2,760.
    let scarred = Utilisation {
        realised_loss: d("120"),
        ..unscathed
    };
    assert_eq!(
        envelope.admit(&venue(), d("1010"), &scarred, at(10)),
        CapitalAdmission::Full,
        "premise: the money limits still admit it"
    );
    assert!(matches!(
        envelope.check_exposure(d("10"), d("1000"), d("1010"), &scarred),
        Err(ExposureBreach::Leverage(_))
    ));
}

#[test]
fn an_envelope_without_bounds_admits_no_exposure_check() {
    let unbounded = CapitalEnvelope::new(
        StrategyId::new("alpha"),
        "london-1",
        d("3000"),
        d("1500"),
        d("500"),
        vec![venue()],
        at(0),
        at(3600),
        "alice@example.com",
        "",
    )
    .unwrap();
    assert!(matches!(
        unbounded.check_exposure(d("1"), d("1000"), d("10"), &Utilisation::default()),
        Err(ExposureBreach::Unbounded(_))
    ));
}
