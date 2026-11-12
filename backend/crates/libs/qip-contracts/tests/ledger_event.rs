//! The ledger event contract's paper fence, from outside the crate.

// In a test the assertion is the deliverable; an expect is the failure report.
#![allow(clippy::expect_used)]

use qip_contracts::ledger::{
    ATTRIBUTED_SCHEMA, Account, Attribution, Direction, FeeReport, LEGACY_SCHEMA, LedgerEvent,
    Posting, Settlement, SettlementState, SourceEvent,
};
use qip_contracts::reflex::Decision;
use qip_core::{Timestamp, dec};
use std::collections::BTreeMap;

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
        source(),
        Settlement::Simulated,
        FeeReport::Unreported,
        vec![leg(Direction::Debit), leg(Direction::Credit)],
    )
    .expect("a balanced event")
}

fn source() -> SourceEvent {
    SourceEvent {
        cell: "cell-1".to_string(),
        session: 1,
        journal_sequence: 4,
        journal_digest: "d4".to_string(),
        order_id: "order-9".to_string(),
    }
}

/// A fill of 3 split 2 to `alpha` and 1 to `beta`, booked to cell-1's
/// trading books against venue `v`.
fn fill_event() -> LedgerEvent {
    let leg = |account: Account, direction, amount| Posting {
        account,
        direction,
        unit: "BTC".to_string(),
        amount,
    };
    let book = |strategy: &str| Account::Trading {
        cell: "cell-1".to_string(),
        strategy: strategy.to_string(),
    };
    let venue = Account::Venue {
        venue: "v".to_string(),
    };
    LedgerEvent::new(
        source(),
        Settlement::Simulated,
        FeeReport::Unreported,
        vec![
            leg(book("alpha"), Direction::Debit, dec!("2")),
            leg(venue.clone(), Direction::Credit, dec!("2")),
            leg(book("beta"), Direction::Debit, dec!("1")),
            leg(venue, Direction::Credit, dec!("1")),
        ],
    )
    .expect("a balanced fill")
}

fn chain() -> Attribution {
    Attribution {
        order_id: "order-9".to_string(),
        decision_sequence: 2,
        contributors: BTreeMap::from([
            ("alpha".to_string(), dec!("2")),
            ("beta".to_string(), dec!("1")),
        ]),
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
        !text.contains("owner") && !text.contains("attribution") && !text.contains("schema"),
        "an unowned event must re-encode as it always did: {text}"
    );
    let back: LedgerEvent = serde_json::from_str(&text).expect("a stored event still decodes");
    assert_eq!(back, bare);

    assert!(
        event().with_owner("").is_err(),
        "a blank owner reads as stated and names nobody"
    );
    assert!(event().with_owner("desk/a").is_err());

    let mut blank = chain();
    blank.order_id.clear();
    assert!(event().with_attribution(blank).is_err(), "a blank order");
    let mut elsewhere = chain();
    elsewhere.order_id = "order-10".to_string();
    assert!(
        event().with_attribution(elsewhere).is_err(),
        "an attribution naming another order than the source was accepted"
    );
    let mut after = chain();
    after.decision_sequence = 4;
    assert!(
        event().with_attribution(after).is_err(),
        "a decision that does not precede its fill was accepted"
    );
    let mut nobody = chain();
    nobody.contributors.clear();
    assert!(event().with_attribution(nobody).is_err(), "no contributor");
    let mut unnamed = chain();
    unnamed.contributors.insert(String::new(), dec!("1"));
    assert!(
        event().with_attribution(unnamed).is_err(),
        "a blank strategy"
    );
    let mut nothing = chain();
    nothing.contributors.insert("gamma".to_string(), dec!("0"));
    assert!(event().with_attribution(nothing).is_err(), "a zero share");

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

/// A sale's cash leg held in settlement: Dr in_settlement, Cr venue.
fn in_settlement_legs() -> Vec<Posting> {
    vec![
        Posting {
            account: Account::InSettlement {
                cell: "cell-1".to_string(),
                strategy: "alpha".to_string(),
            },
            direction: Direction::Debit,
            unit: "USD".to_string(),
            amount: dec!("100"),
        },
        Posting {
            account: Account::Venue {
                venue: "sim-xnys".to_string(),
            },
            direction: Direction::Credit,
            unit: "USD".to_string(),
            amount: dec!("100"),
        },
    ]
}

#[test]
fn an_in_settlement_posting_must_state_its_stage_and_a_stage_must_have_one() {
    // R12-LEDGER-5011: each ledger event carries its settlement state, and a
    // reader is never told cash was booked at once when it waits on the
    // venue. Mutation verified: removing the `in_settlement` refusal from
    // `LedgerEvent::new` admits the unstaged event and fails the first
    // assertion; removing the "posts to no in_settlement account" refusal
    // from `staged` admits the second.
    let staged = LedgerEvent::staged(
        source(),
        Settlement::Simulated,
        FeeReport::Unreported,
        in_settlement_legs(),
        SettlementState::InSettlement,
    )
    .expect("premise: a staged in-settlement event is admitted");
    assert_eq!(staged.settlement_state(), SettlementState::InSettlement);

    assert!(
        LedgerEvent::new(
            source(),
            Settlement::Simulated,
            FeeReport::Unreported,
            in_settlement_legs(),
        )
        .is_err(),
        "an in-settlement posting was admitted with no stage"
    );
    assert!(
        LedgerEvent::staged(
            source(),
            Settlement::Simulated,
            FeeReport::Unreported,
            event().postings().to_vec(),
            SettlementState::InSettlement,
        )
        .is_err(),
        "a stage with nothing in settlement was admitted"
    );
    // A settlement names the fills it closed, ascending, never none.
    for fills in [vec![], vec![3, 3], vec![5, 2]] {
        assert!(
            LedgerEvent::staged(
                source(),
                Settlement::Simulated,
                FeeReport::Unreported,
                in_settlement_legs(),
                SettlementState::Settled {
                    settlement_date: Timestamp::from_secs(1_790_172_800),
                    fills: fills.clone(),
                },
            )
            .is_err(),
            "a settlement naming fills {fills:?} was admitted"
        );
    }
}

#[test]
fn a_stage_round_trips_and_an_event_written_before_stages_re_encodes_byte_for_byte() {
    // The failure this prevents: a stored event whose stage is lost on
    // read, or an old event that re-encodes with a new key and moves every
    // digest over it. Mutation verified: dropping `skip_serializing_if` on
    // `settlement_state` writes a `settlement_state` key into the old
    // event and fails the first assertion; reading the wire without routing
    // the stage through `staged` reads the settled event back as immediate.
    let old = event();
    let old_text = serde_json::to_string(&old).expect("serialises");
    assert!(!old_text.contains("settlement_state"));
    let reread: LedgerEvent = serde_json::from_str(&old_text).expect("reads back");
    assert_eq!(reread.settlement_state(), SettlementState::Immediate);
    assert_eq!(
        serde_json::to_string(&reread).expect("serialises"),
        old_text
    );

    let state = SettlementState::Settled {
        settlement_date: Timestamp::from_secs(1_790_172_800),
        fills: vec![2, 4],
    };
    let settled = LedgerEvent::staged(
        source(),
        Settlement::Simulated,
        FeeReport::Unreported,
        in_settlement_legs(),
        state.clone(),
    )
    .expect("admitted");
    let text = serde_json::to_string(&settled).expect("serialises");
    let back: LedgerEvent = serde_json::from_str(&text).expect("reads back");
    assert_eq!(back.settlement_state(), state);
    assert_eq!(
        back.settlement(),
        Settlement::Simulated,
        "the paper fence moved"
    );

    // An in-settlement posting stored with its stage stripped is refused on
    // read, not read as immediate.
    let stripped = text.replace(
        &format!(
            ",\"settlement_state\":{}",
            serde_json::to_string(&state).expect("serialises")
        ),
        "",
    );
    assert_ne!(stripped, text, "premise: the stage was stripped");
    assert!(serde_json::from_str::<LedgerEvent>(&stripped).is_err());
}

#[test]
fn the_in_settlement_account_reads_back_from_its_text_and_holds_either_sign() {
    // Mutation verified: dropping the `in_settlement` arm of `FromStr`
    // refuses the text and fails the round trip; adding the account to
    // `holds_a_quantity` fails the last assertion, and would make the
    // ledger refuse every purchase at a settling venue.
    let account = Account::InSettlement {
        cell: "cell-1".to_string(),
        strategy: "alpha".to_string(),
    };
    assert_eq!(account.to_string(), "in_settlement:cell-1/alpha");
    assert_eq!(
        "in_settlement:cell-1/alpha"
            .parse::<Account>()
            .expect("parses"),
        account
    );
    assert!("in_settlement:cell-1".parse::<Account>().is_err());
    assert!(!account.holds_a_quantity());
}

#[test]
fn a_settled_decision_is_an_order_outcome_with_its_own_kind() {
    let decision = Decision::Settled {
        order_id: "order-9".to_string(),
        venue: "sim-xnys".to_string(),
        settlement_date: Timestamp::from_secs(1_790_172_800),
        simulated: true,
    };
    assert_eq!(decision.kind(), "settled");
    assert_eq!(decision.order_id(), Some("order-9"));
    let text = serde_json::to_string(&decision).expect("serialises");
    assert_eq!(
        serde_json::from_str::<Decision>(&text).expect("reads back"),
        decision
    );
}

/// R12-CONTRACT-7069 / ADR 0454: a schema 2 event states every required
/// field, decoding one without its owner or its chain is refused, and a
/// schema 1 event still decodes as legacy and re-encodes byte for byte.
///
/// Mutations: let `ATTRIBUTED_SCHEMA` decode with a missing owner (the
/// `let (Some(owner), Some(attribution))` guard falls back to legacy) — the
/// missing-owner decode is accepted; drop the booked-versus-named check in
/// `LedgerEvent::attributed` — the chain naming a strategy the postings never
/// booked is accepted.
#[test]
fn a_schema_two_fill_event_missing_any_required_field_is_refused_and_round_trips_whole() {
    let event = fill_event()
        .attributed("cell-1", chain())
        .expect("a fill owned by its cell with its full chain is booked at schema 2");
    assert_eq!(event.schema(), ATTRIBUTED_SCHEMA);
    assert_eq!(event.owner(), Some("cell-1"));
    assert_eq!(
        event.intent_ids(),
        vec![
            "cell-1/1/2/alpha".to_string(),
            "cell-1/1/2/beta".to_string()
        ]
    );

    // Round trip preserves every field: the economic event, its postings,
    // settlement, fee, owner, chain and schema.
    let text = serde_json::to_string(&event).expect("encodes");
    let value: serde_json::Value = serde_json::from_str(&text).expect("json");
    for field in [
        "schema",
        "source",
        "settlement",
        "fee",
        "postings",
        "owner",
        "attribution",
    ] {
        assert!(value.get(field).is_some(), "{field} missing from {text}");
    }
    let back: LedgerEvent = serde_json::from_str(&text).expect("decodes");
    assert_eq!(back, event);
    assert_eq!(back.schema(), ATTRIBUTED_SCHEMA);
    assert_eq!(serde_json::to_string(&back).expect("re-encodes"), text);

    // Each required field removed from the stored text is refused, not read
    // as legacy.
    for field in [
        "owner",
        "attribution",
        "source",
        "settlement",
        "fee",
        "postings",
    ] {
        let mut stripped = value.clone();
        stripped.as_object_mut().expect("an object").remove(field);
        assert!(
            serde_json::from_value::<LedgerEvent>(stripped).is_err(),
            "a schema 2 event without `{field}` was accepted"
        );
    }
    let mut unknown = value.clone();
    unknown["schema"] = serde_json::json!(3);
    assert!(serde_json::from_value::<LedgerEvent>(unknown).is_err());

    // Construction refuses a chain that does not describe the event.
    let mut extra = chain();
    extra.contributors.insert("gamma".to_string(), dec!("1"));
    assert!(
        fill_event().attributed("cell-1", extra).is_err(),
        "a contributor the postings never booked was accepted"
    );
    let mut short = chain();
    short.contributors.remove("beta");
    assert!(
        fill_event().attributed("cell-1", short).is_err(),
        "a strategy booked but not named in the chain was accepted"
    );
    assert!(
        fill_event().attributed("cell-2", chain()).is_err(),
        "an owner other than the cell whose book the fill moved was accepted"
    );
    assert!(
        event_without_trading().is_err(),
        "a schema 2 event with no trading book was accepted"
    );

    // Schema 1 stays legacy: the unowned fill decodes, keeps schema 1, and
    // re-encodes byte for byte.
    let legacy = fill_event();
    let stored = serde_json::to_string(&legacy).expect("encodes");
    let read: LedgerEvent = serde_json::from_str(&stored).expect("legacy decodes");
    assert_eq!(read.schema(), LEGACY_SCHEMA);
    assert_eq!(serde_json::to_string(&read).expect("re-encodes"), stored);
}

fn event_without_trading() -> qip_core::error::Result<LedgerEvent> {
    event().attributed("treasury-1", chain())
}
