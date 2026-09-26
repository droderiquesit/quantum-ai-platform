//! The ledger event contract's paper fence, from outside the crate.

use qip_contracts::ledger::Settlement;

#[test]
fn settlement_has_no_variant_but_simulated() {
    // ADR 0100 §9's fourth fence is a type: a ledger event's settlement can
    // say `simulated` and nothing else. A second variant — `settled`, `live`,
    // anything a live fill could be booked under — would be accepted from any
    // stream or store that carried it, and nothing downstream would notice.
    // So this reads the wire text every plausible second variant would take,
    // and refuses each.
    //
    // Premise first: the one variant is admitted and writes as `simulated`,
    // so a refusal below is a refusal of the word, not of the format.
    let admitted: Settlement =
        serde_json::from_str("\"simulated\"").expect("premise: `simulated` is admitted");
    assert_eq!(admitted, Settlement::Simulated);
    assert_eq!(
        serde_json::to_string(&Settlement::Simulated).expect("serialises"),
        "\"simulated\""
    );

    for candidate in [
        "settled",
        "live",
        "real",
        "pending",
        "cleared",
        "confirmed",
        "failed",
        "unsettled",
        "Settled",
        "Live",
        "Simulated",
    ] {
        let text = format!("\"{candidate}\"");
        assert!(
            serde_json::from_str::<Settlement>(&text).is_err(),
            "Settlement admitted `{candidate}`; the ledger can now represent a fill that \
             was not simulated"
        );
    }
}
