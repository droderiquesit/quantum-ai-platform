//! The peer half of the Reflex Mesh: epochs, fencing, FIRE, bounds.
//!
//! Each test asserts its own premise first, so a fixture that quietly became
//! empty or already-refused cannot make a refusal test pass.

#![allow(clippy::panic_in_result_fn, clippy::unwrap_used)]

use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp};
use qip_mesh::peer::{
    EpochDraft, EpochLeg, OpportunityEpoch, PeerEndpoint, PeerLimits, PeerMessage, ReservationBook,
    decode, encode,
};

const OPENED: i64 = 1_760_000_000_000_000_000;

fn d(n: i64) -> Decimal {
    Decimal::from_int(n)
}
fn ceiling() -> Duration {
    Duration::from_millis(500)
}
fn limits() -> PeerLimits {
    PeerLimits {
        max_frame: 4096,
        ttl_ceiling: ceiling(),
    }
}
fn at(ms: i64) -> Timestamp {
    Timestamp::from_nanos(OPENED + ms * 1_000_000)
}
fn leg(id: &str, node: &str, unwind: i64) -> EpochLeg {
    EpochLeg {
        leg: id.into(),
        node: node.into(),
        size: d(100),
        unwind_loss: d(unwind),
    }
}
fn draft() -> EpochDraft {
    EpochDraft {
        opportunity: Some("opp-1".into()),
        sequence: Some(1),
        cycle: Some("usd>eur>usd".into()),
        legs: Some(vec![leg("a", "americas", 2), leg("b", "europe", 3)]),
        size: Some(d(100)),
        min_edge_bps: Some(d(5)),
        model_versions: Some(vec!["m-7".into()]),
        opened_at_nanos: Some(OPENED),
        ttl: Some(Duration::from_millis(200)),
        max_recovery_loss: Some(d(10)),
    }
}
fn epoch() -> OpportunityEpoch {
    draft().build(ceiling()).unwrap()
}
fn token(e: &OpportunityEpoch) -> Vec<u8> {
    encode(&PeerMessage::Token { epoch: e.clone() }, 4096).unwrap()
}
fn reserved(e: &OpportunityEpoch, l: &str, seq: u64) -> Vec<u8> {
    let amount = e.legs.iter().find(|x| x.leg == l).unwrap().size;
    encode(
        &PeerMessage::Reserved {
            opportunity: e.opportunity.clone(),
            sequence: seq,
            leg: l.into(),
            amount,
        },
        4096,
    )
    .unwrap()
}
fn endpoint_with(e: &OpportunityEpoch) -> PeerEndpoint {
    let mut ep = PeerEndpoint::new("americas", limits());
    ep.receive(&token(e), at(0)).unwrap();
    ep
}
fn open_gate(_: &EpochLeg) -> Result<()> {
    Ok(())
}

// ---- MESH-005 -------------------------------------------------------------

#[test]
fn an_epoch_missing_any_one_of_its_seven_fields_is_refused_naming_that_field() {
    assert!(
        draft().build(ceiling()).is_ok(),
        "premise: the full draft builds"
    );
    let cases: Vec<(&str, EpochDraft)> = vec![
        (
            "cycle",
            EpochDraft {
                cycle: None,
                ..draft()
            },
        ),
        (
            "legs",
            EpochDraft {
                legs: None,
                ..draft()
            },
        ),
        (
            "size",
            EpochDraft {
                size: None,
                ..draft()
            },
        ),
        (
            "min_edge_bps",
            EpochDraft {
                min_edge_bps: None,
                ..draft()
            },
        ),
        (
            "model_versions",
            EpochDraft {
                model_versions: None,
                ..draft()
            },
        ),
        (
            "ttl",
            EpochDraft {
                ttl: None,
                ..draft()
            },
        ),
        (
            "unwind_policy",
            EpochDraft {
                max_recovery_loss: None,
                ..draft()
            },
        ),
    ];
    for (field, dr) in cases {
        let err = dr.build(ceiling()).unwrap_err().to_string();
        assert!(err.contains(&format!("`{field}`")), "{field}: {err}");
    }
}

#[test]
fn a_token_survives_a_round_trip_and_epochs_differing_in_any_field_have_different_identities() {
    let e = epoch();
    let back = decode(&token(&e), 4096).unwrap();
    assert_eq!(back, PeerMessage::Token { epoch: e.clone() });

    let variants = vec![
        EpochDraft {
            opportunity: Some("opp-2".into()),
            ..draft()
        },
        EpochDraft {
            sequence: Some(2),
            ..draft()
        },
        EpochDraft {
            cycle: Some("other".into()),
            ..draft()
        },
        EpochDraft {
            legs: Some(vec![leg("a", "americas", 2), leg("b", "apac", 3)]),
            ..draft()
        },
        EpochDraft {
            size: Some(d(101)),
            ..draft()
        },
        EpochDraft {
            min_edge_bps: Some(d(6)),
            ..draft()
        },
        EpochDraft {
            model_versions: Some(vec!["m-8".into()]),
            ..draft()
        },
        EpochDraft {
            opened_at_nanos: Some(OPENED + 1),
            ..draft()
        },
        EpochDraft {
            ttl: Some(Duration::from_millis(201)),
            ..draft()
        },
        EpochDraft {
            max_recovery_loss: Some(d(11)),
            ..draft()
        },
    ];
    let mut ids = std::collections::BTreeSet::new();
    ids.insert(e.identity());
    for v in variants {
        ids.insert(v.build(ceiling()).unwrap().identity());
    }
    assert_eq!(ids.len(), 11, "every field change must change the identity");
}

// ---- MESH-022 -------------------------------------------------------------

#[test]
fn an_epoch_with_no_ttl_or_a_ttl_above_the_ceiling_is_refused_and_one_at_the_ceiling_is_admitted() {
    let at_ceiling = EpochDraft {
        ttl: Some(ceiling()),
        ..draft()
    };
    assert!(
        at_ceiling.build(ceiling()).is_ok(),
        "premise: the ceiling itself is admitted"
    );
    let over = EpochDraft {
        ttl: Some(Duration::from_millis(501)),
        ..draft()
    };
    assert!(
        over.build(ceiling())
            .unwrap_err()
            .to_string()
            .contains("ceiling")
    );
    let zero = EpochDraft {
        ttl: Some(Duration::from_millis(0)),
        ..draft()
    };
    assert!(zero.build(ceiling()).is_err());
    let none = EpochDraft {
        ttl: None,
        ..draft()
    };
    assert!(
        none.build(ceiling())
            .unwrap_err()
            .to_string()
            .contains("`ttl`")
    );
}

#[test]
fn no_leg_is_sent_and_no_message_is_applied_at_or_after_the_epoch_expiry() {
    let e = epoch();
    let mut ep = endpoint_with(&e);
    ep.receive(&reserved(&e, "a", 1), at(10)).unwrap();
    ep.receive(&reserved(&e, "b", 1), at(10)).unwrap();
    // Late FIRE: the instant of expiry is already too late.
    assert!(ep.declare_fire("opp-1", at(200)).is_err());
    assert!(!ep.is_fired("opp-1"));
    let fire = ep.declare_fire("opp-1", at(199)).unwrap();
    assert!(ep.is_fired("opp-1"), "premise: FIRE in time works");
    assert!(ep.send_leg("opp-1", "a", at(200), &open_gate).is_err());
    assert!(!ep.leg_sent("opp-1", "a"));
    ep.send_leg("opp-1", "a", at(199), &open_gate).unwrap();
    assert!(ep.leg_sent("opp-1", "a"));
    // A late message bearing the epoch is discarded and journaled.
    let before = ep.refusals().len();
    assert!(ep.receive(&fire, at(250)).is_err());
    assert_eq!(ep.refusals().len(), before + 1);
    assert_eq!(ep.refusals()[before].kind, "expired");
}

// ---- MESH-021 -------------------------------------------------------------

#[test]
fn a_message_from_a_superseded_epoch_changes_nothing_and_is_journaled_as_refused() {
    let old = epoch();
    let newer = EpochDraft {
        sequence: Some(2),
        ..draft()
    }
    .build(ceiling())
    .unwrap();
    let mut ep = endpoint_with(&old);
    ep.receive(&token(&newer), at(5)).unwrap();
    assert_eq!(
        ep.current_sequence("opp-1"),
        Some(2),
        "premise: epoch 2 is current"
    );

    // A node that restarted holding epoch 1 reserves, then replays its token.
    assert!(ep.receive(&reserved(&old, "a", 1), at(6)).is_err());
    assert!(ep.receive(&token(&old), at(7)).is_err());
    assert_eq!(
        ep.reserved("opp-1", "a"),
        None,
        "stale reservation must not apply"
    );
    assert_eq!(
        ep.current_sequence("opp-1"),
        Some(2),
        "stale token must not roll back"
    );
    let kinds: Vec<_> = ep.refusals().iter().map(|r| r.kind).collect();
    assert_eq!(kinds, vec!["stale_epoch", "stale_epoch"]);

    // Current-epoch traffic still applies.
    ep.receive(&reserved(&newer, "a", 2), at(8)).unwrap();
    assert_eq!(ep.reserved("opp-1", "a"), Some(d(100)));
}

// ---- MESH-029 -------------------------------------------------------------

#[test]
fn fire_is_declared_exactly_when_every_leg_is_reserved_and_no_leg_precedes_it() {
    let e = epoch();
    // Every combination of which of the two legs reserved, refused or timed out.
    for mask in 0u8..4 {
        let mut ep = endpoint_with(&e);
        for (i, l) in ["a", "b"].iter().enumerate() {
            if mask & (1 << i) != 0 {
                ep.receive(&reserved(&e, l, 1), at(1)).unwrap();
            }
        }
        let all = mask == 3;
        assert_eq!(ep.declare_fire("opp-1", at(2)).is_ok(), all, "mask {mask}");
        assert_eq!(ep.is_fired("opp-1"), all, "mask {mask}");
        for l in ["a", "b"] {
            let sent = ep.send_leg("opp-1", l, at(3), &open_gate);
            assert_eq!(sent.is_ok(), all && l == "a", "mask {mask} leg {l}");
        }
    }
}

#[test]
fn a_leg_reserved_for_less_than_its_requirement_does_not_meet_the_fire_condition() {
    let e = epoch();
    let mut ep = endpoint_with(&e);
    ep.receive(&reserved(&e, "a", 1), at(1)).unwrap();
    let partial = encode(
        &PeerMessage::Reserved {
            opportunity: "opp-1".into(),
            sequence: 1,
            leg: "b".into(),
            amount: d(40),
        },
        4096,
    )
    .unwrap();
    ep.receive(&partial, at(1)).unwrap();
    assert_eq!(
        ep.reserved("opp-1", "b"),
        Some(d(40)),
        "premise: partial hold recorded"
    );
    assert!(ep.declare_fire("opp-1", at(2)).is_err());
}

// ---- MESH-042 -------------------------------------------------------------

#[test]
fn a_message_at_the_size_bound_is_sent_and_one_byte_over_is_refused_by_sender_and_receiver() {
    let msg = PeerMessage::Token { epoch: epoch() };
    let exact = serde_json::to_vec(&msg).unwrap().len();
    let frame = encode(&msg, exact).expect("exactly at the bound encodes");
    assert!(
        decode(&frame, exact).is_ok(),
        "exactly at the bound is accepted"
    );
    assert!(
        encode(&msg, exact - 1).is_err(),
        "sender refuses one byte over"
    );
    assert!(
        decode(&frame, exact - 1).is_err(),
        "receiver refuses one byte over"
    );

    // A hostile frame declaring 4 GiB with no body is refused on its prefix.
    let hostile = u32::MAX.to_be_bytes().to_vec();
    let err = decode(&hostile, 4096).unwrap_err().to_string();
    assert!(err.contains("refused unread"), "{err}");
}

// ---- MESH-044 -------------------------------------------------------------

#[test]
fn reservations_never_exceed_the_leg_or_the_grant_and_are_released_when_the_epoch_ends() {
    let prior = d(240);
    // Generated epochs: deterministic sizes so a failure is reproducible.
    for n in 1..=40i64 {
        let mut book = ReservationBook::new(prior);
        let mut e = epoch();
        e.opportunity = format!("opp-{n}");
        e.legs[0].size = d(50 + n);
        e.legs[1].size = d(200 - n);
        assert!(
            book.reserve(&e, "a", e.legs[0].size + d(1)).is_err(),
            "above the leg's requirement, though the grant has room"
        );
        assert_eq!(book.available(), prior, "premise: the refusal held nothing");
        let a = book.reserve(&e, "a", e.legs[0].size);
        let b = book.reserve(&e, "b", e.legs[1].size);
        assert!(a.is_ok(), "premise: the first leg fits the grant");
        // The two legs need 250 against a grant of 240: the second must be refused.
        assert!(b.is_err(), "second leg exceeds the remaining grant");
        assert_eq!(book.available(), prior - e.legs[0].size);
        book.release_epoch(&e.opportunity);
        assert_eq!(book.available(), prior, "completed epoch returns the grant");
    }
}

#[test]
fn a_reservation_is_released_at_its_epoch_expiry_and_not_a_moment_before() {
    let e = epoch();
    let mut book = ReservationBook::new(d(250));
    book.reserve(&e, "a", d(100)).unwrap();
    book.expire(at(199));
    assert_eq!(
        book.available(),
        d(150),
        "premise: still held before expiry"
    );
    book.expire(at(200));
    assert_eq!(book.available(), d(250));
}

// ---- MESH-009 -------------------------------------------------------------

#[test]
fn fire_is_refused_when_the_worst_recovery_loss_exceeds_the_bound_declared_before_it() {
    let tight = EpochDraft {
        max_recovery_loss: Some(d(4)),
        ..draft()
    }
    .build(ceiling())
    .unwrap();
    let mut ep = endpoint_with(&tight);
    ep.receive(&reserved(&tight, "a", 1), at(1)).unwrap();
    ep.receive(&reserved(&tight, "b", 1), at(1)).unwrap();
    // Premise: every leg is reserved, so only the loss bound can refuse.
    assert!(ep.reserved("opp-1", "a").is_some() && ep.reserved("opp-1", "b").is_some());
    let err = ep.declare_fire("opp-1", at(2)).unwrap_err().to_string();
    assert!(err.contains("worst recovery loss"), "{err}");
    assert!(!ep.is_fired("opp-1"));
}

#[test]
fn realized_recovery_loss_never_passes_the_bound_over_every_order_of_unwinds() {
    let e = epoch();
    let bound = e.max_recovery_loss;
    // Every subset of legs stranded, unwound in both orders, at the declared cost.
    for order in [["a", "b"], ["b", "a"]] {
        let mut ep = endpoint_with(&e);
        ep.receive(&reserved(&e, "a", 1), at(1)).unwrap();
        ep.receive(&reserved(&e, "b", 1), at(1)).unwrap();
        ep.declare_fire("opp-1", at(2)).unwrap();
        for l in order {
            let cost = e.legs.iter().find(|x| x.leg == l).unwrap().unwind_loss;
            let msg = PeerMessage::Unwind {
                opportunity: "opp-1".into(),
                sequence: 1,
                leg: l.into(),
                realized_loss: cost,
            };
            ep.receive(&encode(&msg, 4096).unwrap(), at(3)).unwrap();
            assert!(ep.realized_recovery_loss("opp-1") <= bound);
        }
        assert_eq!(
            ep.realized_recovery_loss("opp-1"),
            d(5),
            "premise: both unwound"
        );
    }
}

#[test]
fn an_unwind_costing_more_than_the_leg_declared_is_refused_and_not_booked() {
    let e = epoch();
    let mut ep = endpoint_with(&e);
    ep.receive(&reserved(&e, "a", 1), at(1)).unwrap();
    ep.receive(&reserved(&e, "b", 1), at(1)).unwrap();
    ep.declare_fire("opp-1", at(2)).unwrap();
    let msg = PeerMessage::Unwind {
        opportunity: "opp-1".into(),
        sequence: 1,
        leg: "a".into(),
        realized_loss: d(3),
    };
    assert!(ep.receive(&encode(&msg, 4096).unwrap(), at(3)).is_err());
    assert_eq!(ep.realized_recovery_loss("opp-1"), d(0));
    assert_eq!(ep.refusals().last().map(|r| r.kind), Some("loss_bound"));
}

// ---- MESH-003 -------------------------------------------------------------

#[test]
fn a_well_formed_pack_policy_or_grant_delivered_by_a_peer_is_refused_journaled_and_changes_nothing()
{
    let e = epoch();
    let mut ep = endpoint_with(&e);
    let kinds = [
        "policy",
        "policy_pack",
        "capital_grant",
        "risk_pack",
        "model_pack",
        "strategy_pack",
        "belief_pack",
    ];
    for k in kinds {
        let body = format!(r#"{{"kind":"{k}","payload":"signed-bytes","signature":"ok"}}"#);
        let mut frame = (body.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(body.as_bytes());
        let err = ep.receive(&frame, at(1)).unwrap_err().to_string();
        assert!(err.contains("never from a peer"), "{k}: {err}");
    }
    assert_eq!(
        ep.refusals().len(),
        kinds.len(),
        "every refusal is journaled"
    );
    assert_eq!(ep.current_sequence("opp-1"), Some(1), "state unchanged");
    assert_eq!(ep.reserved("opp-1", "a"), None);
}

// ---- MESH-045 -------------------------------------------------------------

#[test]
fn a_fired_leg_the_local_risk_gate_vetoes_is_never_sent() {
    let e = epoch();
    let mut ep = endpoint_with(&e);
    ep.receive(&reserved(&e, "a", 1), at(1)).unwrap();
    ep.receive(&reserved(&e, "b", 1), at(1)).unwrap();
    ep.declare_fire("opp-1", at(2)).unwrap();
    assert!(ep.is_fired("opp-1"), "premise: FIRE was declared");
    let veto = |l: &EpochLeg| -> Result<()> {
        Err(qip_core::error::Error::denied(format!(
            "limit breached on {}",
            l.leg
        )))
    };
    assert!(ep.send_leg("opp-1", "a", at(3), &veto).is_err());
    assert!(!ep.leg_sent("opp-1", "a"));
    ep.send_leg("opp-1", "a", at(3), &open_gate).unwrap();
    assert!(
        ep.leg_sent("opp-1", "a"),
        "the same leg goes when the gate admits it"
    );
}
