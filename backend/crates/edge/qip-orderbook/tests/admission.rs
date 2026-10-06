//! Order-level capture needs a licence entitlement and a cost justification.

#![allow(clippy::panic_in_result_fn)]

#[allow(dead_code)]
mod common;

use common::{instrument, l3_stream, venue};
use qip_contracts::VenueStatus;
use qip_core::error::Result;
use qip_orderbook::BookView;
use qip_orderbook::admission::DepthAdmissions;

fn config(entitlement: Option<&str>, justification: Option<&str>) -> String {
    let field = |k: &str, v: Option<&str>| v.map_or(String::new(), |v| format!(r#","{k}":"{v}""#));
    format!(
        r#"[{{"venue":"XNAS","depth":"order_by_order"{}{}}}]"#,
        field("l3_entitlement", entitlement),
        field("l3_cost_justification", justification),
    )
}

#[test]
fn loading_refuses_order_level_capture_unless_both_facts_are_recorded() -> Result<()> {
    let both = config(
        Some("totalview"),
        Some("queue position drives passive fills"),
    );
    assert!(
        DepthAdmissions::load(&both).is_ok(),
        "premise: both present admits"
    );

    for (name, text) in [
        ("no entitlement", config(None, Some("worth it"))),
        ("no justification", config(Some("totalview"), None)),
        ("neither", config(None, None)),
        ("blank entitlement", config(Some("  "), Some("worth it"))),
        ("blank justification", config(Some("totalview"), Some(""))),
    ] {
        let refusal = DepthAdmissions::load(&text).expect_err(name);
        assert!(refusal.to_string().contains("XNAS"), "{name}: {refusal}");
    }
    Ok(())
}

#[test]
fn an_admitted_venue_captures_order_level_events_and_an_unlisted_one_is_not_admitted() -> Result<()>
{
    let admissions = DepthAdmissions::load(&config(Some("totalview"), Some("queue position")))?;
    let admission = admissions.for_venue(&venue()).expect("XNAS admitted");
    assert_eq!(admission.entitlement(), "totalview");

    let mut state = admission.open_state(instrument(), VenueStatus::Open);
    for message in l3_stream(5, 500) {
        state.apply(&message)?;
    }
    assert!(
        state
            .book()
            .as_order_by_order()
            .expect("order-by-order")
            .resting_orders()
            > 20,
        "the admitted adapter tracks individual orders"
    );

    let other = qip_contracts::VenueId::new("XNYS");
    assert!(admissions.for_venue(&other).is_none());
    Ok(())
}

#[test]
fn order_level_terms_on_an_aggregated_venue_and_duplicates_are_refused() -> Result<()> {
    let agg = r#"[{"venue":"XNAS","depth":"aggregated"}]"#;
    let admissions = DepthAdmissions::load(agg)?;
    assert!(
        admissions.for_venue(&venue()).is_none(),
        "aggregated is not an L3 admission"
    );

    let stray = r#"[{"venue":"XNAS","depth":"aggregated","l3_entitlement":"x"}]"#;
    assert!(DepthAdmissions::load(stray).is_err());
    let twice = format!(
        "[{0},{0}]",
        &config(Some("e"), Some("j"))[1..config(Some("e"), Some("j")).len() - 1]
    );
    assert!(DepthAdmissions::load(&twice).is_err());
    assert!(DepthAdmissions::load(r#"[{"venue":"XNAS","depth":"l3"}]"#).is_err());
    Ok(())
}
