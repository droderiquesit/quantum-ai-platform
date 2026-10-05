//! Peer rules added with the clock bound, the fan-out bound and forgetting:
//! MESH-007, MESH-041, MESH-017. Each test asserts its premise first.

#![allow(clippy::panic_in_result_fn, clippy::unwrap_used)]

use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp};
use qip_mesh::peer::{
    EpochDraft, EpochLeg, OpportunityEpoch, PeerEndpoint, PeerLimits, PeerMessage, encode,
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
        max_clock_offset: Duration::from_millis(5),
        max_fan_out: 2,
    }
}
fn at(ms: i64) -> Timestamp {
    Timestamp::from_nanos(OPENED + ms * 1_000_000)
}
fn leg(id: &str, node: &str) -> EpochLeg {
    EpochLeg {
        leg: id.into(),
        node: node.into(),
        size: d(100),
        unwind_loss: d(1),
    }
}
fn epoch_over(legs: Vec<EpochLeg>) -> OpportunityEpoch {
    EpochDraft {
        opportunity: Some("opp-1".into()),
        sequence: Some(1),
        cycle: Some("c".into()),
        legs: Some(legs),
        size: Some(d(100)),
        min_edge_bps: Some(d(5)),
        model_versions: Some(vec!["m-7".into()]),
        opened_at_nanos: Some(OPENED),
        ttl: Some(Duration::from_millis(200)),
        max_recovery_loss: Some(d(10)),
    }
    .build(ceiling())
    .unwrap()
}
fn epoch() -> OpportunityEpoch {
    epoch_over(vec![leg("a", "americas"), leg("b", "europe")])
}
fn token(e: &OpportunityEpoch) -> Vec<u8> {
    encode(&PeerMessage::Token { epoch: e.clone() }, 4096).unwrap()
}
fn reserved(e: &OpportunityEpoch, l: &str) -> Vec<u8> {
    let amount = e.legs.iter().find(|x| x.leg == l).unwrap().size;
    encode(
        &PeerMessage::Reserved {
            opportunity: e.opportunity.clone(),
            sequence: e.sequence,
            leg: l.into(),
            amount,
        },
        4096,
    )
    .unwrap()
}
fn endpoint_with(e: &OpportunityEpoch) -> PeerEndpoint {
    let mut ep = PeerEndpoint::new("americas", limits());
    ep.observe_clock_offset(0);
    ep.receive(&token(e), at(0)).unwrap();
    ep
}
fn open_gate(_: &EpochLeg) -> Result<()> {
    Ok(())
}

#[test]
fn a_node_whose_clock_offset_is_unknown_or_beyond_the_bound_does_not_join_an_epoch() {
    let e = epoch();
    let bound_ns = limits().max_clock_offset.as_nanos();
    assert!(bound_ns > 0, "premise: the bound is a real, positive one");

    // Unknown offset: refused, not read as zero.
    let mut unknown = PeerEndpoint::new("americas", limits());
    assert!(unknown.receive(&token(&e), at(0)).is_err());
    assert_eq!(unknown.refusals().last().unwrap().kind, "clock_unknown");
    assert_eq!(unknown.held_opportunities(), 0);

    // Beyond the bound in either direction: refused. At the bound: admitted.
    for off in [bound_ns + 1, -(bound_ns + 1)] {
        let mut ep = PeerEndpoint::new("americas", limits());
        ep.observe_clock_offset(off);
        assert!(ep.receive(&token(&e), at(0)).is_err());
        assert_eq!(ep.refusals().last().unwrap().kind, "clock_out_of_bound");
        assert_eq!(ep.held_opportunities(), 0);
    }
    for off in [bound_ns, -bound_ns] {
        let mut ep = PeerEndpoint::new("americas", limits());
        ep.observe_clock_offset(off);
        assert!(ep.receive(&token(&e), at(0)).is_ok());
    }
}

#[test]
fn a_node_whose_clock_drifts_out_of_bound_after_joining_neither_fires_nor_sends() {
    let e = epoch();
    let mut ep = endpoint_with(&e);
    ep.receive(&reserved(&e, "a"), at(1)).unwrap();
    ep.receive(&reserved(&e, "b"), at(1)).unwrap();
    ep.observe_clock_offset(limits().max_clock_offset.as_nanos() + 1);
    assert!(ep.declare_fire("opp-1", at(2)).is_err());
    assert!(!ep.is_fired("opp-1"), "premise: nothing fired");
    ep.observe_clock_offset(0);
    ep.declare_fire("opp-1", at(2)).unwrap();
    ep.clear_clock_offset();
    assert!(ep.send_leg("opp-1", "a", at(3), &open_gate).is_err());
    assert!(!ep.leg_sent("opp-1", "a"), "premise: nothing was sent");
    ep.observe_clock_offset(0);
    ep.send_leg("opp-1", "a", at(3), &open_gate).unwrap();
}

#[test]
fn an_update_goes_only_to_the_peers_holding_a_leg_and_never_past_the_fan_out_bound() {
    // Two legs on two nodes: one peer besides americas.
    let ep = endpoint_with(&epoch());
    assert_eq!(ep.recipients("opp-1").unwrap(), vec!["europe".to_string()]);

    // Three peers with a bound of two: refused whole, not shortened.
    let wide = epoch_over(vec![
        leg("a", "americas"),
        leg("b", "europe"),
        leg("c", "apac"),
        leg("d", "africa"),
    ]);
    let peers = wide.legs.iter().filter(|l| l.node != "americas").count();
    assert!(peers > limits().max_fan_out, "premise: over the bound");
    assert!(endpoint_with(&wide).recipients("opp-1").is_err());

    // Exactly at the bound is admitted, in order, without duplicates.
    let at_bound = epoch_over(vec![
        leg("a", "americas"),
        leg("b", "europe"),
        leg("c", "apac"),
        leg("d", "apac"),
    ]);
    assert_eq!(
        endpoint_with(&at_bound).recipients("opp-1").unwrap(),
        vec!["apac".to_string(), "europe".to_string()]
    );
}

#[test]
fn an_expired_epoch_is_forgotten_and_a_late_frame_for_it_is_refused_not_replayed() {
    let e = epoch();
    let mut ep = endpoint_with(&e);
    assert_eq!(ep.held_opportunities(), 1, "premise: the epoch is held");
    ep.forget_expired(at(199));
    assert_eq!(ep.held_opportunities(), 1, "not forgotten before expiry");
    ep.forget_expired(at(200));
    assert_eq!(ep.held_opportunities(), 0);
    assert!(ep.receive(&reserved(&e, "a"), at(201)).is_err());
    assert_eq!(ep.refusals().last().unwrap().kind, "unknown_opportunity");
}
