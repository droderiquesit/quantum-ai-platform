//! Three rules of the peer protocol that were prose until this suite:
//! the lane each message kind runs in (MESH-013), the local gate running
//! before anything is reserved (MESH-028), and a cycle that cannot have its
//! guarantees being rejected by name rather than dropped (MESH-036).
//!
//! The frames here cross between two real endpoints through `encode` and
//! `receive`, so what the coordinator learns is what a participant actually
//! put on the wire, not a value the test handed it directly.

#![allow(clippy::panic_in_result_fn, clippy::unwrap_used, clippy::expect_used)]

use qip_core::error::{Error, Result};
use qip_core::{Decimal, Duration, Timestamp};
use qip_mesh::peer::{
    EpochDraft, EpochLeg, Lane, MeshFunction, OpportunityEpoch, PeerEndpoint, PeerLimits,
    PeerMessage, ReservationBook, ReservationOutcome, decode, encode,
};
use std::collections::BTreeSet;

const OPENED: i64 = 1_760_000_000_000_000_000;
const MAX_FRAME: usize = 4096;

fn d(n: i64) -> Decimal {
    Decimal::from_int(n)
}
fn ceiling() -> Duration {
    Duration::from_millis(500)
}
fn limits() -> PeerLimits {
    PeerLimits {
        max_frame: MAX_FRAME,
        ttl_ceiling: ceiling(),
        max_clock_offset: Duration::from_millis(5),
        max_fan_out: 2,
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
fn epoch_over(legs: Vec<EpochLeg>, max_recovery_loss: i64) -> OpportunityEpoch {
    EpochDraft {
        opportunity: Some("opp-1".into()),
        sequence: Some(1),
        cycle: Some("usd>eur>usd".into()),
        legs: Some(legs),
        size: Some(d(100)),
        min_edge_bps: Some(d(5)),
        model_versions: Some(vec!["m-7".into()]),
        opened_at_nanos: Some(OPENED),
        ttl: Some(Duration::from_millis(200)),
        max_recovery_loss: Some(d(max_recovery_loss)),
    }
    .build(ceiling())
    .unwrap()
}
fn epoch() -> OpportunityEpoch {
    epoch_over(vec![leg("a", "americas", 2), leg("b", "europe", 3)], 10)
}
fn token(e: &OpportunityEpoch) -> Vec<u8> {
    encode(&PeerMessage::Token { epoch: e.clone() }, MAX_FRAME).unwrap()
}
/// A node that has measured its clock and been told of the epoch.
fn node_holding(name: &str, e: &OpportunityEpoch) -> PeerEndpoint {
    let mut ep = PeerEndpoint::new(name, limits());
    ep.observe_clock_offset(0);
    ep.receive(&token(e), at(0)).unwrap();
    ep
}
fn open_gate(_: &EpochLeg) -> Result<()> {
    Ok(())
}
fn kinds(ep: &PeerEndpoint) -> Vec<&'static str> {
    ep.refusals().iter().map(|r| r.kind).collect()
}

// ---- MESH-013 -------------------------------------------------------------

#[test]
fn each_of_the_five_coordination_functions_is_registered_in_lane_one_and_every_message_kind_belongs_to_one()
 {
    // The failure this prevents: a message kind added to the peer protocol
    // with no lane decided for it, which a Lane 0 pass could then be written
    // to wait on without any register saying it must not.
    let e = epoch();
    let every_kind = vec![
        PeerMessage::Token { epoch: e.clone() },
        PeerMessage::Reserved {
            opportunity: "opp-1".into(),
            sequence: 1,
            leg: "a".into(),
            amount: d(100),
        },
        PeerMessage::Declined {
            opportunity: "opp-1".into(),
            sequence: 1,
            leg: "a".into(),
            reason: "limit".into(),
        },
        PeerMessage::Fire {
            opportunity: "opp-1".into(),
            sequence: 1,
        },
        PeerMessage::Fill {
            opportunity: "opp-1".into(),
            sequence: 1,
            leg: "a".into(),
            quantity: d(100),
        },
        PeerMessage::Unwind {
            opportunity: "opp-1".into(),
            sequence: 1,
            leg: "a".into(),
            realized_loss: d(1),
        },
    ];

    // Premise: the list above really is every kind on the wire. Each one
    // round-trips, and the tags are distinct, so a kind missing from this
    // test shows up as a tag count that no longer matches the enum's.
    let mut tags = BTreeSet::new();
    for message in &every_kind {
        let frame = encode(message, MAX_FRAME).unwrap();
        assert_eq!(&decode(&frame, MAX_FRAME).unwrap(), message);
        let body: serde_json::Value = serde_json::from_slice(&frame[4..]).unwrap();
        tags.insert(body["kind"].as_str().unwrap().to_string());
    }
    assert_eq!(
        tags.len(),
        every_kind.len(),
        "premise: every kind listed has its own wire tag"
    );

    // Premise: the registry holds five functions, the five §4 names.
    assert_eq!(MeshFunction::ALL.len(), 5);
    let names: BTreeSet<&str> = MeshFunction::ALL.iter().map(|f| f.as_str()).collect();
    assert_eq!(
        names,
        BTreeSet::from([
            "peer_reflex_message",
            "opportunity_token",
            "distributed_reservation",
            "multi_leg_coordination",
            "hedge_unwind_command",
        ])
    );

    // The registration: all five are Lane 1, none is Lane 0.
    for function in MeshFunction::ALL {
        assert_eq!(
            function.lane(),
            Lane::CoordinatedFast,
            "{} is not registered in Lane 1",
            function.as_str()
        );
        assert_eq!(function.lane().number(), 1);
        assert_ne!(function.lane(), Lane::Reflex);
    }

    // Every kind belongs to a registered function and so to Lane 1, and
    // between them the kinds reach all five functions: none of the five is a
    // name with no message behind it.
    let mut reached = BTreeSet::new();
    for message in &every_kind {
        assert!(MeshFunction::ALL.contains(&message.function()));
        assert_eq!(message.lane(), Lane::CoordinatedFast);
        reached.insert(message.function());
    }
    assert_eq!(
        reached,
        MeshFunction::ALL.into_iter().collect::<BTreeSet<_>>(),
        "a registered function has no message kind"
    );
}

// ---- MESH-028 -------------------------------------------------------------

#[test]
fn a_node_whose_gate_refuses_a_leg_reserves_nothing_and_its_refusal_reaches_the_coordinator_and_blocks_fire()
 {
    // Europe holds two legs; its gate admits the first and refuses the
    // second. Reserving leg by leg, gate then book, would leave the first
    // leg's capital held for a cycle this node has just refused.
    let e = epoch_over(
        vec![
            leg("a", "americas", 2),
            leg("b1", "europe", 3),
            leg("b2", "europe", 3),
        ],
        10,
    );
    let mut americas = node_holding("americas", &e);
    let mut europe = node_holding("europe", &e);
    let mut americas_book = ReservationBook::new(d(1000));
    let mut europe_book = ReservationBook::new(d(1000));

    // Premise: the epoch is held by both, nothing is reserved anywhere, and
    // the book could afford both of Europe's legs — so only the gate can be
    // what refuses.
    assert_eq!(europe.current_sequence("opp-1"), Some(1));
    assert_eq!(europe_book.available(), d(1000));
    assert!(europe_book.available() >= d(200));

    // The coordinator reserves its own leg and tells itself nothing more.
    let own = americas
        .reserve_own_legs("opp-1", at(1), &mut americas_book, &open_gate)
        .unwrap();
    assert!(matches!(own, ReservationOutcome::Reserved(ref frames) if frames.len() == 1));
    assert_eq!(americas.reserved("opp-1", "a"), Some(d(100)));

    let breach = |l: &EpochLeg| -> Result<()> {
        if l.leg == "b2" {
            Err(Error::denied("position limit breached on b2"))
        } else {
            Ok(())
        }
    };
    let outcome = europe
        .reserve_own_legs("opp-1", at(2), &mut europe_book, &breach)
        .unwrap();
    let ReservationOutcome::Declined(frame) = outcome else {
        panic!("a leg the gate refused was reserved: {outcome:?}");
    };

    // The node reserved nothing: not the refused leg, and not the leg the
    // gate had already admitted.
    assert_eq!(europe_book.available(), d(1000));
    assert_eq!(europe.reserved("opp-1", "b1"), None);
    assert_eq!(europe.reserved("opp-1", "b2"), None);
    assert_eq!(kinds(&europe), vec!["gate_refused"]);

    // The refusal is a frame, and it names the leg and carries the reason.
    match decode(&frame, MAX_FRAME).unwrap() {
        PeerMessage::Declined {
            opportunity,
            sequence,
            leg,
            reason,
        } => {
            assert_eq!(
                (opportunity.as_str(), sequence, leg.as_str()),
                ("opp-1", 1, "b2")
            );
            assert!(reason.contains("position limit breached on b2"), "{reason}");
        }
        other => panic!("the node sent {other:?} instead of a decline"),
    }

    // It reaches the coordinator, is journaled there, and FIRE is not
    // declared.
    let before = americas.refusals().len();
    americas.receive(&frame, at(3)).unwrap();
    assert_eq!(americas.refusals().len(), before + 1);
    let journaled = &americas.refusals()[before];
    assert_eq!(journaled.kind, "peer_declined");
    assert!(
        journaled.reason.contains("`b2`") && journaled.reason.contains("position limit breached"),
        "{}",
        journaled.reason
    );
    let refused = americas
        .declare_fire("opp-1", at(4))
        .unwrap_err()
        .to_string();
    assert!(refused.contains("declined by its node"), "{refused}");
    assert!(!americas.is_fired("opp-1"));

    // A reservation for the declined leg arriving afterwards does not undo
    // the decline: the epoch stays unfireable until a new token.
    for l in ["b1", "b2"] {
        let late = encode(
            &PeerMessage::Reserved {
                opportunity: "opp-1".into(),
                sequence: 1,
                leg: l.into(),
                amount: d(100),
            },
            MAX_FRAME,
        )
        .unwrap();
        let applied = americas.receive(&late, at(5));
        assert_eq!(applied.is_ok(), l == "b1", "leg {l}");
    }
    assert!(americas.declare_fire("opp-1", at(6)).is_err());
    assert!(!americas.is_fired("opp-1"));

    // The other half: the same node with a gate that admits both legs
    // reserves both, and the coordinator fires. Without this the test would
    // pass against a participant that declined everything.
    let mut americas = node_holding("americas", &e);
    let mut europe = node_holding("europe", &e);
    let mut europe_book = ReservationBook::new(d(1000));
    americas
        .reserve_own_legs(
            "opp-1",
            at(1),
            &mut ReservationBook::new(d(1000)),
            &open_gate,
        )
        .unwrap();
    let ReservationOutcome::Reserved(frames) = europe
        .reserve_own_legs("opp-1", at(2), &mut europe_book, &open_gate)
        .unwrap()
    else {
        panic!("an admitted node declined");
    };
    assert_eq!(frames.len(), 2);
    assert_eq!(europe_book.available(), d(800));
    for frame in &frames {
        americas.receive(frame, at(3)).unwrap();
    }
    americas.declare_fire("opp-1", at(4)).unwrap();
    assert!(americas.is_fired("opp-1"));
}

#[test]
fn a_node_whose_book_cannot_hold_every_one_of_its_legs_holds_none_of_them_and_declines() {
    // A grant that covers one of two legs. Whole or not at all: the leg the
    // book did accept is released again, or the node would be left holding
    // half a reservation for a cycle that can never fire.
    let e = epoch_over(
        vec![
            leg("a", "americas", 2),
            leg("b1", "europe", 3),
            leg("b2", "europe", 3),
        ],
        10,
    );
    let mut europe = node_holding("europe", &e);
    let mut book = ReservationBook::new(d(150));
    assert!(
        book.available() >= d(100) && book.available() < d(200),
        "premise: the grant covers exactly one of the two legs"
    );
    let outcome = europe
        .reserve_own_legs("opp-1", at(1), &mut book, &open_gate)
        .unwrap();
    assert!(
        matches!(outcome, ReservationOutcome::Declined(_)),
        "{outcome:?}"
    );
    assert_eq!(book.available(), d(150), "the first leg's hold was kept");
    assert_eq!(europe.reserved("opp-1", "b1"), None);
    assert_eq!(kinds(&europe), vec!["reservation_refused"]);
}

// ---- MESH-036 -------------------------------------------------------------

#[test]
fn a_cycle_whose_reservation_condition_is_not_met_within_its_ttl_is_rejected_by_name_and_sends_no_leg()
 {
    let e = epoch();
    let mut coordinator = node_holding("americas", &e);
    let mut book = ReservationBook::new(d(1000));
    coordinator
        .reserve_own_legs("opp-1", at(1), &mut book, &open_gate)
        .unwrap();

    // Premise: the epoch is live and held, the coordinator's own leg is
    // reserved, Europe's never arrives, and nothing has been refused yet —
    // so every journal entry below is one this test caused.
    assert_eq!(coordinator.held_opportunities(), 1);
    assert_eq!(coordinator.reserved("opp-1", "a"), Some(d(100)));
    assert_eq!(coordinator.reserved("opp-1", "b"), None);
    assert!(coordinator.refusals().is_empty());

    // FIRE inside the TTL: refused, and the refusal is in the journal
    // naming the guarantee and the leg, not only in the returned error.
    assert!(coordinator.declare_fire("opp-1", at(50)).is_err());
    assert!(!coordinator.is_fired("opp-1"));
    assert_eq!(kinds(&coordinator), vec!["fire_condition"]);
    let reason = &coordinator.refusals()[0].reason;
    assert!(
        reason.contains("reservation condition") && reason.contains("`b`"),
        "{reason}"
    );
    assert!(
        !reason.contains("`a`"),
        "a reserved leg was named as missing: {reason}"
    );

    // No leg goes while the condition is unmet, and that is journaled too.
    assert!(
        coordinator
            .send_leg("opp-1", "a", at(60), &open_gate)
            .is_err()
    );
    assert!(!coordinator.leg_sent("opp-1", "a"));
    assert_eq!(kinds(&coordinator), vec!["fire_condition", "before_fire"]);

    // One instant before expiry the cycle is still only pending.
    coordinator.forget_expired(at(199));
    assert_eq!(coordinator.held_opportunities(), 1);
    assert_eq!(coordinator.refusals().len(), 2);

    // At expiry it is rejected, by name, and gone.
    coordinator.forget_expired(at(200));
    assert_eq!(coordinator.held_opportunities(), 0);
    assert_eq!(
        kinds(&coordinator),
        vec!["fire_condition", "before_fire", "guarantee_unmet"]
    );
    let rejection = &coordinator.refusals()[2].reason;
    assert!(
        rejection.contains("`opp-1`")
            && rejection.contains("rejected")
            && rejection.contains("TTL")
            && rejection.contains("reservation condition")
            && rejection.contains("`b`")
            && rejection.contains("no leg was sent"),
        "{rejection}"
    );
    assert!(
        coordinator
            .send_leg("opp-1", "a", at(201), &open_gate)
            .is_err()
    );
    assert!(!coordinator.leg_sent("opp-1", "a"));
}

#[test]
fn a_cycle_whose_loss_bound_cannot_be_established_is_rejected_by_name_and_sends_no_leg() {
    // Unwinding both legs can cost 2 + 3; the epoch declared it would lose
    // at most 4.
    let tight = epoch_over(vec![leg("a", "americas", 2), leg("b", "europe", 3)], 4);
    let mut coordinator = node_holding("americas", &tight);
    let mut europe = node_holding("europe", &tight);
    coordinator
        .reserve_own_legs(
            "opp-1",
            at(1),
            &mut ReservationBook::new(d(1000)),
            &open_gate,
        )
        .unwrap();
    let ReservationOutcome::Reserved(frames) = europe
        .reserve_own_legs(
            "opp-1",
            at(1),
            &mut ReservationBook::new(d(1000)),
            &open_gate,
        )
        .unwrap()
    else {
        panic!("premise: Europe reserves");
    };
    for frame in &frames {
        coordinator.receive(frame, at(2)).unwrap();
    }

    // Premise: the reservation condition is met in full, so the loss bound
    // is the only guarantee that can be missing.
    assert_eq!(coordinator.reserved("opp-1", "a"), Some(d(100)));
    assert_eq!(coordinator.reserved("opp-1", "b"), Some(d(100)));
    assert!(coordinator.refusals().is_empty());

    assert!(coordinator.declare_fire("opp-1", at(3)).is_err());
    assert!(!coordinator.is_fired("opp-1"));
    assert_eq!(kinds(&coordinator), vec!["fire_loss_bound"]);
    let reason = &coordinator.refusals()[0].reason;
    assert!(
        reason.contains("loss bound cannot be established")
            && reason.contains('5')
            && reason.contains('4'),
        "{reason}"
    );
    assert!(
        coordinator
            .send_leg("opp-1", "a", at(4), &open_gate)
            .is_err()
    );
    assert!(!coordinator.leg_sent("opp-1", "a"));

    // And at the end of its TTL the cycle is rejected rather than dropped.
    coordinator.forget_expired(at(200));
    assert_eq!(coordinator.held_opportunities(), 0);
    assert_eq!(kinds(&coordinator).last().copied(), Some("guarantee_unmet"));

    // The other half: the same cycle with a bound that covers the worst
    // recovery fires. Without it this test passes against a coordinator
    // that never fires anything.
    let covered = epoch_over(vec![leg("a", "americas", 2), leg("b", "europe", 3)], 5);
    let mut coordinator = node_holding("americas", &covered);
    let mut europe = node_holding("europe", &covered);
    coordinator
        .reserve_own_legs(
            "opp-1",
            at(1),
            &mut ReservationBook::new(d(1000)),
            &open_gate,
        )
        .unwrap();
    let ReservationOutcome::Reserved(frames) = europe
        .reserve_own_legs(
            "opp-1",
            at(1),
            &mut ReservationBook::new(d(1000)),
            &open_gate,
        )
        .unwrap()
    else {
        panic!("premise: Europe reserves");
    };
    for frame in &frames {
        coordinator.receive(frame, at(2)).unwrap();
    }
    coordinator.declare_fire("opp-1", at(3)).unwrap();
    coordinator
        .send_leg("opp-1", "a", at(4), &open_gate)
        .unwrap();
    assert!(coordinator.leg_sent("opp-1", "a"));
    // A cycle that fired and then reached its expiry is not a rejection.
    coordinator.forget_expired(at(200));
    assert!(
        !kinds(&coordinator).contains(&"guarantee_unmet"),
        "a fired cycle was journaled as rejected: {:?}",
        coordinator.refusals()
    );
}
