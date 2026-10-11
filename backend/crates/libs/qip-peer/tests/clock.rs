//! MESH-007 by measurement: a node learns each peer's clock offset from a
//! four-timestamp probe and echo, and joins an epoch only while every
//! counterparty's offset, plus its uncertainty, is inside the bound.
//!
//! Before the probe existed a node had no way to measure anything, so every
//! token a production node received was refused as `clock_unknown` — the
//! rule was sound and the mesh could never coordinate a cycle. These tests
//! hand frames between endpoints in memory with chosen instants, so the
//! arithmetic is checked against offsets the test itself set.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::{Decimal, Duration, Timestamp};
use qip_peer::{
    CLOCK_SAMPLE_TTL, EpochDraft, EpochLeg, LegExecution, LegSide, OpportunityEpoch, PeerEndpoint,
    PeerLimits, PeerMessage, encode,
};

const BASE: i64 = 1_760_000_000_000_000_000;
const MS: i64 = 1_000_000;

fn at(ns: i64) -> Timestamp {
    Timestamp::from_nanos(BASE + ns)
}

fn limits() -> PeerLimits {
    PeerLimits {
        max_frame: 64 * 1024,
        ttl_ceiling: Duration::from_secs(30),
        max_clock_offset: Duration::from_millis(5),
        max_fan_out: 8,
    }
}

fn epoch(opened: Timestamp) -> OpportunityEpoch {
    let leg = |id: &str, node: &str| EpochLeg {
        leg: id.into(),
        node: node.into(),
        size: Decimal::from_int(10),
        unwind_loss: Decimal::from_int(1),
        execution: Some(LegExecution {
            venue: "XLON".into(),
            instrument: "obj-ACME".into(),
            side: if id == "buy" {
                LegSide::Buy
            } else {
                LegSide::Sell
            },
            limit_price: Decimal::from_int(100),
        }),
    };
    EpochDraft {
        opportunity: Some("opp-1".into()),
        sequence: Some(1),
        cycle: Some("obj-ACME:XLON>XNYS".into()),
        legs: Some(vec![leg("buy", "london"), leg("sell", "newyork")]),
        size: Some(Decimal::from_int(10)),
        min_edge_bps: Some(Decimal::from_int(5)),
        model_versions: Some(vec!["peer-touch/1".into()]),
        opened_at_nanos: Some(opened.as_nanos()),
        ttl: Some(Duration::from_secs(2)),
        max_recovery_loss: Some(Decimal::from_int(2)),
    }
    .build(Duration::from_secs(30))
    .expect("the draft builds")
}

/// Run one probe from `prober` to `peer`. `peer_ahead_ns` is how far the
/// peer's clock runs ahead of the prober's; `one_way_ns` is each direction's
/// path delay. Returns the instant, on the prober's clock, the echo landed.
fn measure(
    prober: &mut PeerEndpoint,
    peer: &mut PeerEndpoint,
    sent: i64,
    peer_ahead_ns: i64,
    one_way_ns: i64,
) -> Result<Timestamp> {
    let probe = prober.clock_probe_frame(at(sent))?;
    let arrived_at_peer = at(sent + one_way_ns + peer_ahead_ns);
    peer.receive_from(&probe, prober.node(), arrived_at_peer)?;
    let echoes = peer.take_echoes(arrived_at_peer);
    assert_eq!(echoes.len(), 1, "premise: the peer owes exactly one echo");
    assert_eq!(
        echoes[0].0,
        prober.node(),
        "premise: addressed to the prober"
    );
    let landed = at(sent + 2 * one_way_ns);
    prober.receive_from(&echoes[0].1, peer.node(), landed)?;
    Ok(landed)
}

/// Mutation: drop the `(t3 - t4)` term from the offset (or store a zero
/// offset) and the measured figure stops matching the offset set here.
#[test]
fn a_probe_and_its_echo_measure_the_offset_the_peer_clock_actually_has() -> Result<()> {
    let mut london = PeerEndpoint::new("london", limits());
    let mut newyork = PeerEndpoint::new("newyork", limits());
    assert!(
        london.peer_clock("newyork", at(0)).is_none(),
        "premise: nothing is measured before a probe"
    );
    let landed = measure(&mut london, &mut newyork, 0, 3 * MS, MS)?;
    let sample = london
        .peer_clock("newyork", landed)
        .expect("the echo produced a sample");
    assert_eq!(sample.offset_nanos, 3 * MS, "{sample:?}");
    assert_eq!(sample.round_trip_nanos, 2 * MS, "{sample:?}");
    assert_eq!(sample.worst_offset_nanos(), (4 * MS) as u64);
    Ok(())
}

/// The fix this suite exists for. With no measurement the token is refused
/// as `clock_unknown` naming the peer; once the peer is measured inside the
/// bound the same token installs; a peer measured outside it is refused as
/// `clock_out_of_bound`; and a sample past its TTL is unknown again.
///
/// Mutation: make `clock_in_bound_for` accept a counterparty with no sample
/// (treat absent as zero) and the first assertion fails; make it ignore the
/// uncertainty term and the out-of-bound case installs.
#[test]
fn a_token_is_admitted_only_while_every_counterpartys_clock_is_measured_and_in_bound() -> Result<()>
{
    let token = encode(
        &PeerMessage::Token {
            epoch: epoch(at(0)),
        },
        64 * 1024,
    )?;

    let mut london = PeerEndpoint::new("london", limits());
    let mut newyork = PeerEndpoint::new("newyork", limits());
    assert!(newyork.receive_from(&token, "london", at(MS)).is_err());
    let refusal = newyork.refusals().last().expect("journaled");
    assert_eq!(refusal.kind, "clock_unknown");
    assert!(refusal.reason.contains("`london`"), "{}", refusal.reason);

    // Measured: london's clock 1 ms behind newyork's, ±1 ms.
    let landed = measure(&mut newyork, &mut london, 2 * MS, -MS, MS)?;
    newyork.receive_from(&token, "london", landed)?;
    assert_eq!(newyork.current_sequence("opp-1"), Some(1));

    // A third node whose peer is 4.5 ms off with a 2 ms round trip: the
    // estimate alone is inside 5 ms and the estimate plus its uncertainty is
    // not, so it is refused.
    let mut boston = PeerEndpoint::new("newyork", limits());
    let mut far = PeerEndpoint::new("london", limits());
    let landed = measure(&mut boston, &mut far, 0, 4 * MS + MS / 2, MS)?;
    assert!(boston.receive_from(&token, "london", landed).is_err());
    assert_eq!(boston.refusals().last().unwrap().kind, "clock_out_of_bound");

    // Stale: the in-bound sample above ages past its TTL.
    let mut late = PeerEndpoint::new("newyork", limits());
    let mut peer = PeerEndpoint::new("london", limits());
    let landed = measure(&mut late, &mut peer, 0, 0, MS)?;
    let after_ttl = Timestamp::from_nanos(landed.as_nanos() + CLOCK_SAMPLE_TTL.as_nanos() + 1);
    let fresh_token = encode(
        &PeerMessage::Token {
            epoch: epoch(after_ttl),
        },
        64 * 1024,
    )?;
    assert!(
        late.receive_from(&fresh_token, "london", after_ttl)
            .is_err()
    );
    assert_eq!(late.refusals().last().unwrap().kind, "clock_unknown");
    Ok(())
}

/// An echo that answers no outstanding probe sets nothing: a replayed or
/// forged echo would otherwise set whatever offset its author chose.
///
/// Mutation: drop the outstanding-probe check in `absorb_echo` and the
/// forged echo yields a sample.
#[test]
fn an_echo_that_answers_no_outstanding_probe_is_refused_and_measures_nothing() -> Result<()> {
    let mut london = PeerEndpoint::new("london", limits());
    let forged = encode(
        &PeerMessage::ClockEcho {
            node: "newyork".into(),
            to: "london".into(),
            nonce: 99,
            origin_ns: at(0).as_nanos(),
            received_ns: at(MS).as_nanos(),
            sent_ns: at(MS).as_nanos(),
        },
        64 * 1024,
    )?;
    assert!(london.receive_from(&forged, "newyork", at(2 * MS)).is_err());
    assert_eq!(london.refusals().last().unwrap().kind, "unknown_probe");
    assert!(london.peer_clock("newyork", at(2 * MS)).is_none());
    Ok(())
}

/// With an authenticated sender, a frame speaks only for that sender: an
/// edge state under a neighbour's name, a reservation for a leg another node
/// holds, and a FIRE from a node that did not coordinate are each refused.
///
/// Mutation: make `sender_is` always succeed (or drop the FIRE coordinator
/// check) and the corresponding frame is admitted.
#[test]
fn an_authenticated_peer_cannot_speak_for_another_node() -> Result<()> {
    let mut london = PeerEndpoint::new("london", limits());
    let mut newyork = PeerEndpoint::new("newyork", limits());
    london.observe_clock_offset(0);
    newyork.observe_clock_offset(0);

    let edge_state = encode(
        &PeerMessage::EdgeState {
            node: "tokyo".into(),
            sequence: 1,
            edges: vec![],
        },
        64 * 1024,
    )?;
    assert!(london.receive_from(&edge_state, "newyork", at(0)).is_err());
    assert_eq!(london.refusals().last().unwrap().kind, "sender_mismatch");
    // Premise: the same frame from its own node is admitted.
    london.receive_from(&edge_state, "tokyo", at(0))?;

    let token = london.open_epoch(epoch(at(0)), at(0))?;
    assert!(london.coordinates("opp-1"));
    newyork.receive_from(&token, "london", at(MS))?;
    assert!(!newyork.coordinates("opp-1"));

    // newyork reports a reservation for london's leg: refused.
    let forged_reserve = encode(
        &PeerMessage::Reserved {
            opportunity: "opp-1".into(),
            sequence: 1,
            leg: "buy".into(),
            amount: Decimal::from_int(10),
        },
        64 * 1024,
    )?;
    assert!(
        london
            .receive_from(&forged_reserve, "newyork", at(2 * MS))
            .is_err()
    );
    assert_eq!(london.refusals().last().unwrap().kind, "sender_mismatch");
    assert_eq!(london.reserved("opp-1", "buy"), None);

    // A FIRE from the participant: refused at the participant's peer.
    let fire = encode(
        &PeerMessage::Fire {
            opportunity: "opp-1".into(),
            sequence: 1,
        },
        64 * 1024,
    )?;
    assert!(newyork.receive_from(&fire, "tokyo", at(3 * MS)).is_err());
    assert_eq!(newyork.refusals().last().unwrap().kind, "sender_mismatch");
    Ok(())
}

/// A node coordinates only a cycle it trades in.
///
/// Mutation: drop the own-leg check in `open_epoch` and the epoch installs.
#[test]
fn a_node_with_no_leg_in_an_epoch_cannot_open_it() {
    let mut tokyo = PeerEndpoint::new("tokyo", limits());
    tokyo.observe_clock_offset(0);
    assert!(tokyo.open_epoch(epoch(at(0)), at(0)).is_err());
    assert_eq!(tokyo.refusals().last().unwrap().kind, "no_own_leg");
    assert_eq!(tokyo.held_opportunities(), 0);
}
