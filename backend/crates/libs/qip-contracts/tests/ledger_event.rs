//! The ledger event contract's paper fence, from outside the crate.

// In a test the assertion is the deliverable; an expect is the failure report.
#![allow(clippy::expect_used)]

use qip_contracts::ledger::{
    Account, Attribution, Direction, FeeReport, LedgerEvent, Posting, Settlement, SourceEvent,
};
use qip_core::dec;

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

fn event() -> LedgerEvent {
    let leg = |direction| Posting {
        account: Account::Treasury {
            treasury: "treasury-1".to_string(),
        },
        direction,
        unit: "USD".to_string(),
        amount: dec!("10"),
    };
    LedgerEvent::new(
        SourceEvent {
            cell: "cell-1".to_string(),
            session: 1,
            journal_sequence: 4,
            journal_digest: "d4".to_string(),
            order_id: "order-9".to_string(),
        },
        Settlement::Simulated,
        FeeReport::Unreported,
        vec![leg(Direction::Debit), leg(Direction::Credit)],
    )
    .expect("a balanced event")
}

fn chain() -> Attribution {
    Attribution {
        fill_id: "fill-1".to_string(),
        order_id: "order-9".to_string(),
        intent_id: "intent-3".to_string(),
        decision_id: "decision-2".to_string(),
    }
}

/// Mutation: drop the `refuse_unnamed("owner", ..)` call in
/// `LedgerEvent::with_owner` — the blank owner is accepted.
#[test]
fn an_owner_and_an_attribution_chain_are_refused_when_blank_and_round_trip_when_stated() {
    let bare = event();
    assert_eq!(
        bare.owner(),
        None,
        "premise: an event states no owner until told"
    );
    assert_eq!(bare.attribution(), None);
    let text = serde_json::to_string(&bare).expect("encodes");
    assert!(
        !text.contains("owner") && !text.contains("attribution"),
        "an unowned event must re-encode as it always did: {text}"
    );
    let back: LedgerEvent = serde_json::from_str(&text).expect("a stored event still decodes");
    assert_eq!(back, bare);

    assert!(
        event().with_owner("").is_err(),
        "a blank owner reads as stated and names nobody"
    );
    assert!(event().with_owner("desk/a").is_err());
    for blank in ["fill_id", "order_id", "intent_id", "decision_id"] {
        let mut broken = chain();
        match blank {
            "fill_id" => broken.fill_id.clear(),
            "order_id" => broken.order_id.clear(),
            "intent_id" => broken.intent_id.clear(),
            _ => broken.decision_id.clear(),
        }
        assert!(
            event().with_attribution(broken).is_err(),
            "a chain with a blank {blank} was accepted"
        );
    }
    let mut elsewhere = chain();
    elsewhere.order_id = "order-10".to_string();
    assert!(
        event().with_attribution(elsewhere).is_err(),
        "an attribution naming another order than the source was accepted"
    );

    let owned = event()
        .with_owner("desk-alpha")
        .and_then(|e| e.with_attribution(chain()))
        .expect("a complete statement is accepted");
    assert_eq!(owned.owner(), Some("desk-alpha"));
    assert_eq!(owned.attribution(), Some(&chain()));
    let text = serde_json::to_string(&owned).expect("encodes");
    let back: LedgerEvent = serde_json::from_str(&text).expect("decodes");
    assert_eq!(back, owned);

    // A stored event that states a blank owner meets the same refusal as one
    // built in code.
    let tampered = text.replace("desk-alpha", "");
    assert!(serde_json::from_str::<LedgerEvent>(&tampered).is_err());
}
