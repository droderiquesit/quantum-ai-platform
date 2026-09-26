//! `SchemaId` is content-derived and `check_compatible` is the version-bump
//! gate on top of it (FABRIC-022..025, CONTRACT-046, CONTRACT-047).
//!
//! `prost` is BLOCKED(C2) (ADR 0099), so there is no wire-format compiler to
//! generate a schema identifier from; these tests prove the in-tree
//! substitute holds the two properties a stream will rely on before it ever
//! admits a producer by schema id: the id is a pure function of shape, not
//! of a particular sample's values, and a breaking shape change is refused
//! unless the version says the break was deliberate.

use qip_events::event_fabric::schema_id::{SchemaId, Shape, check_compatible};
use qip_events::{EventBody, SchemaRegistry, Topic};
use serde::{Deserialize, Serialize};

// --- fixtures for the version-bump gate -------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
struct OrderV1 {
    symbol: String,
    price: f64,
}

/// Stands in for both "removed" and "renamed": a structural check cannot
/// tell the two apart, because both leave `price` absent from the new
/// shape. `check_compatible`'s own doc comment says why one fixture proves
/// both.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct OrderPriceGone {
    symbol: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct OrderPriceRetyped {
    symbol: String,
    price: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct OrderNoteAdded {
    symbol: String,
    price: f64,
    note: String,
}

/// Removing, renaming or retyping a field without a version bump is refused,
/// and a field that is only added is admitted without one.
///
/// This is the compatibility gate a stream checks before admitting a
/// producer's schema id: without it, a producer that silently dropped or
/// retyped a field would be indistinguishable from one that hadn't, and a
/// consumer built against the old shape would misread or panic on the new
/// payload instead of the platform refusing the mismatch up front.
#[test]
fn removing_renaming_or_retyping_a_field_without_a_version_bump_is_refused_and_a_defaulted_addition_is_admitted()
 {
    let base = Shape::of(&OrderV1 {
        symbol: "A".into(),
        price: 1.0,
    })
    .unwrap();
    let removed_or_renamed = Shape::of(&OrderPriceGone { symbol: "A".into() }).unwrap();
    let retyped = Shape::of(&OrderPriceRetyped {
        symbol: "A".into(),
        price: "1.0".into(),
    })
    .unwrap();
    let added = Shape::of(&OrderNoteAdded {
        symbol: "A".into(),
        price: 1.0,
        note: "n".into(),
    })
    .unwrap();

    // Premise: the three evolved fixtures really do differ in shape from
    // `base`. A refusal or admission compared against an unchanged shape
    // below would prove nothing.
    assert_ne!(base, removed_or_renamed);
    assert_ne!(base, retyped);
    assert_ne!(base, added);

    assert!(
        check_compatible(1, &base, 1, &removed_or_renamed).is_err(),
        "a field that disappears -- dropped or renamed -- must be refused without a version bump"
    );
    assert!(
        check_compatible(1, &base, 1, &retyped).is_err(),
        "a field that changes kind in place must be refused without a version bump"
    );
    assert!(
        check_compatible(1, &base, 1, &added).is_ok(),
        "a field that is only added must be admitted without a version bump"
    );

    // The same breaking changes are sanctioned once the version says the
    // break was deliberate.
    assert!(check_compatible(1, &base, 2, &removed_or_renamed).is_ok());
    assert!(check_compatible(1, &base, 2, &retyped).is_ok());
}

// --- fixtures for content-derived identity ----------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Quote {
    symbol: String,
    price: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct QuoteWithVenue {
    symbol: String,
    price: f64,
    venue: String,
}

/// Two samples of the same shape, however different their field values, get
/// the same schema id; two samples of different shapes, at the same topic
/// and version, never do.
///
/// A producer's schema id has to be a function of the *shape* it publishes,
/// not the particular values in the sample used to compute it -- otherwise
/// every publish would mint a new id and a stream could never recognise a
/// repeat producer as the one it already admitted.
#[test]
fn identical_content_gets_the_same_schema_id_and_different_content_never_does() {
    let a1 = Shape::of(&Quote {
        symbol: "AAA".into(),
        price: 1.0,
    })
    .unwrap();
    let a2 = Shape::of(&Quote {
        symbol: "ZZZ".into(),
        price: 999.5,
    })
    .unwrap();
    let b = Shape::of(&QuoteWithVenue {
        symbol: "AAA".into(),
        price: 1.0,
        venue: "X".into(),
    })
    .unwrap();

    // Premise: the shape computation itself treats the two `Quote` samples
    // as identical despite their different field values, and treats the
    // `QuoteWithVenue` sample as different. If this were not true, the
    // schema-id comparison below would not be testing what it claims to.
    assert_eq!(
        a1, a2,
        "two samples of the same struct must yield the same Shape regardless of field values"
    );
    assert_ne!(
        a1, b,
        "a sample with an extra field must yield a different Shape"
    );

    let id_a1 = SchemaId::new("market.quote", 1, &a1);
    let id_a2 = SchemaId::new("market.quote", 1, &a2);
    let id_b = SchemaId::new("market.quote", 1, &b);

    assert_eq!(
        id_a1, id_a2,
        "identical shape content must yield the same schema id regardless of the sample's field values"
    );
    assert_ne!(
        id_a1, id_b,
        "different shape content at the same topic and version must never share a schema id"
    );
}

// --- fixtures for the nested-type-change probe ------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
struct OrderRef {
    id: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct OrderRefRetyped {
    id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Fill {
    order: OrderRef,
}

impl EventBody for Fill {
    const TOPIC: Topic = Topic::OrderFilled;
    const SCHEMA_VERSION: u32 = 1;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FillRetyped {
    order: OrderRefRetyped,
}

impl EventBody for FillRetyped {
    const TOPIC: Topic = Topic::OrderFilled;
    const SCHEMA_VERSION: u32 = 1;
}

/// The recursive shape sees a nested type change that the registry's own
/// top-level-field fingerprint (`registry.rs:44`) misses, because that
/// fingerprint hashes only the sorted top-level field names.
///
/// `Fill` and `FillRetyped` both have exactly one top-level field, `order`,
/// so the old fingerprint reads them as identical even though `order.id`
/// changed from a number to a string underneath. A stream admitting
/// producers on that fingerprint alone would treat the retyped producer as
/// the same one it already knew, and every consumer parsing `order.id` as a
/// number would misparse the new payload.
#[test]
fn the_shape_fingerprint_sees_a_nested_type_change_the_top_level_fingerprint_missed() {
    let mut registry_a = SchemaRegistry::new();
    registry_a
        .register(&Fill {
            order: OrderRef { id: 1 },
        })
        .unwrap();
    let mut registry_b = SchemaRegistry::new();
    registry_b
        .register(&FillRetyped {
            order: OrderRefRetyped { id: "1".into() },
        })
        .unwrap();

    let descriptor_a = registry_a.get(Topic::OrderFilled).unwrap();
    let descriptor_b = registry_b.get(Topic::OrderFilled).unwrap();

    // Premise: both registrations really do carry the same, non-empty,
    // top-level field list, so the fingerprint comparison below exercises
    // the case it is meant to miss rather than a vacuous one.
    assert_eq!(descriptor_a.fields, vec!["order".to_string()]);
    assert_eq!(descriptor_a.fields, descriptor_b.fields);

    assert_eq!(
        descriptor_a.fingerprint, descriptor_b.fingerprint,
        "the top-level fingerprint is expected to miss a nested retype -- that is the gap this packet closes"
    );
    assert_ne!(
        descriptor_a.schema_id, descriptor_b.schema_id,
        "the shape-derived schema id must see a nested type change the top-level fingerprint missed"
    );
}
