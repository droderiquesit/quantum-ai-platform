//! The peer Reflex Mesh over a real socket (R12-MESH-5035, 5046, 5060,
//! 5061–5065, 5070).
//!
//! `qip_peer` is proven frame by frame in `qip-mesh`'s suites by
//! handing bytes from one endpoint to another in memory. What those cannot
//! show is that two nodes, each with its own listener and its own wire
//! thread, exchange those frames node to node — not through the centre, and
//! not through the fabric. Every test here sends through `PeerLink` and
//! receives off a socket.
//!
//! Moved from `qip-edge-node/tests/` with the link itself (ADR 0116). The
//! last test drives the link the way the node does in production: through
//! `boundary::ReflexPort` alone, with the session on its own thread.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::error::Result;
use qip_core::{Decimal, Duration, SystemClock, Timestamp, dec};
use qip_peer::{
    EpochDraft, EpochLeg, ExecutableEdge, MAX_EDGES_PER_STATE, OpportunityEpoch, PeerMessage,
    ReservationBook, ReservationOutcome, encode,
};
use qip_peer_link::boundary::EdgeQuote;
use qip_peer_link::{MAX_FRAME, PeerSession, PeerSettings};
use std::collections::BTreeMap;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;

const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);
const OPENED: i64 = 1_760_000_000_000_000_000;

fn at(ms: i64) -> Timestamp {
    Timestamp::from_nanos(OPENED + ms * 1_000_000)
}

/// A loopback port nothing is listening on, at the instant it is returned.
fn free_port() -> SocketAddr {
    let probe = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    probe.local_addr().expect("its address")
}

fn settings(node: &str, listen: SocketAddr, peer: &str, peer_at: SocketAddr) -> PeerSettings {
    PeerSettings {
        node: node.to_string(),
        listen,
        peers: BTreeMap::from([(peer.to_string(), peer_at)]),
    }
}

/// Two sessions, each listening on its own port and naming the other.
fn pair(left: &str, right: &str) -> Result<(PeerSession, PeerSession)> {
    let (left_at, right_at) = (free_port(), free_port());
    Ok((
        PeerSession::new(&settings(left, left_at, right, right_at))?,
        PeerSession::new(&settings(right, right_at, left, left_at))?,
    ))
}

/// Wait for at least one frame on `session`'s wire, apply every frame that
/// arrived to its endpoint, and return how many were applied.
fn deliver(session: &mut PeerSession, now: Timestamp) -> usize {
    let began = std::time::Instant::now();
    loop {
        let frames = session.link().take_inbound();
        if !frames.is_empty() {
            for frame in &frames {
                let _ = session.endpoint_mut().receive(frame, now);
            }
            return frames.len();
        }
        assert!(
            began.elapsed() < PATIENCE,
            "no frame crossed the socket within {PATIENCE:?}: {:?}",
            session.link().stats()
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn leg(id: &str, node: &str, unwind: i64) -> EpochLeg {
    EpochLeg {
        leg: id.into(),
        node: node.into(),
        size: Decimal::from_int(100),
        unwind_loss: Decimal::from_int(unwind),
    }
}

fn epoch() -> OpportunityEpoch {
    EpochDraft {
        opportunity: Some("opp-1".into()),
        sequence: Some(1),
        cycle: Some("usd>eur>usd".into()),
        legs: Some(vec![leg("a", "americas", 2), leg("b", "europe", 3)]),
        size: Some(Decimal::from_int(100)),
        min_edge_bps: Some(Decimal::from_int(5)),
        model_versions: Some(vec!["m-7".into()]),
        opened_at_nanos: Some(OPENED),
        ttl: Some(Duration::from_secs(10)),
        max_recovery_loss: Some(Decimal::from_int(10)),
    }
    .build(Duration::from_secs(30))
    .expect("the full draft builds")
}

fn open_gate(_: &EpochLeg) -> Result<()> {
    Ok(())
}

/// Two nodes carry a distributed cycle through every message kind — token,
/// reserved, FIRE, fill, unwind — each one written to a socket by one node's
/// wire thread and read off a socket by the other's, with the protocol's
/// own rules applied at the receiving end.
///
/// Mutation: make `Wire::send` skip the `write_all` (or drop the `Fill`
/// arm of `PeerEndpoint::admit`) and the step that waits for that frame
/// fails.
#[test]
fn two_nodes_carry_a_cycle_through_token_reserved_fire_fill_and_unwind_over_the_socket()
-> Result<()> {
    let (mut americas, mut europe) = pair("americas", "europe")?;
    americas.endpoint_mut().observe_clock_offset(0);
    europe.endpoint_mut().observe_clock_offset(0);
    let e = epoch();

    // The coordinator holds the token and sends it.
    let token = encode(&PeerMessage::Token { epoch: e.clone() }, MAX_FRAME)?;
    americas.endpoint_mut().receive(&token, at(0))?;
    assert!(americas.send(token));
    deliver(&mut europe, at(1));
    assert_eq!(
        europe.endpoint().current_sequence("opp-1"),
        Some(1),
        "the token did not reach the participant over the socket"
    );

    // Each reserves its own leg; the participant's reservation crosses.
    let mut americas_book = ReservationBook::new(Decimal::from_int(1000));
    let mut europe_book = ReservationBook::new(Decimal::from_int(1000));
    let ReservationOutcome::Reserved(own_frames) =
        americas
            .endpoint_mut()
            .reserve_own_legs("opp-1", at(2), &mut americas_book, &open_gate)?
    else {
        panic!("the coordinator declined an open gate");
    };
    assert_eq!(own_frames.len(), 1, "one leg, one reservation frame");
    let ReservationOutcome::Reserved(frames) =
        europe
            .endpoint_mut()
            .reserve_own_legs("opp-1", at(2), &mut europe_book, &open_gate)?
    else {
        panic!("the participant declined an open gate");
    };
    assert_eq!(frames.len(), 1, "one leg, one reservation frame");
    // Premise: before the frame crosses, the coordinator cannot fire.
    assert!(
        americas
            .endpoint_mut()
            .declare_fire("opp-1", at(3))
            .is_err()
    );
    for frame in frames {
        assert!(europe.send(frame));
    }
    deliver(&mut americas, at(4));
    assert_eq!(
        americas.endpoint().reserved("opp-1", "b"),
        Some(Decimal::from_int(100))
    );
    // And the coordinator's reservation crosses the other way. A participant
    // admits FIRE only once it has seen every leg held — its own view, not
    // the coordinator's word — so until this frame crossed, the FIRE below
    // was refused at the participant as "before every leg is reserved",
    // which is the protocol working and was this test's fixture omitting
    // the coordinator's half of the exchange.
    for frame in own_frames {
        assert!(americas.send(frame));
    }
    deliver(&mut europe, at(4));
    assert_eq!(
        europe.endpoint().reserved("opp-1", "a"),
        Some(Decimal::from_int(100)),
        "the coordinator's reservation did not reach the participant"
    );

    // FIRE, only now that every leg is held, and it crosses.
    let fire = americas.endpoint_mut().declare_fire("opp-1", at(5))?;
    assert!(americas.send(fire));
    deliver(&mut europe, at(6));
    assert!(
        europe.endpoint().is_fired("opp-1"),
        "FIRE did not reach the participant"
    );
    europe
        .endpoint_mut()
        .send_leg("opp-1", "b", at(7), &open_gate)?;

    // The fill streams back peer to peer, then the unwind.
    let fill = encode(
        &PeerMessage::Fill {
            opportunity: "opp-1".into(),
            sequence: 1,
            leg: "b".into(),
            quantity: Decimal::from_int(75),
        },
        MAX_FRAME,
    )?;
    assert!(europe.send(fill));
    deliver(&mut americas, at(8));
    assert_eq!(
        americas.endpoint().filled("opp-1", "b"),
        Some(Decimal::from_int(75))
    );
    let unwind = encode(
        &PeerMessage::Unwind {
            opportunity: "opp-1".into(),
            sequence: 1,
            leg: "b".into(),
            realized_loss: Decimal::from_int(1),
        },
        MAX_FRAME,
    )?;
    assert!(europe.send(unwind));
    deliver(&mut americas, at(9));
    assert_eq!(
        americas.endpoint().realized_recovery_loss("opp-1"),
        Decimal::from_int(1)
    );
    assert!(
        americas.link().stats().received >= 3 && europe.link().stats().received >= 2,
        "the frames counted on the wire do not match what crossed: {:?} {:?}",
        americas.link().stats(),
        europe.link().stats()
    );
    Ok(())
}

fn edge(i: usize) -> ExecutableEdge {
    ExecutableEdge {
        venue: "XLON".into(),
        instrument: format!("obj-{i}"),
        bid: dec!("99"),
        bid_size: dec!("10"),
        ask: dec!("101"),
        ask_size: dec!("10"),
        as_of_ns: OPENED,
    }
}

/// A node publishes the touch of its books — not the books — and a peer holds
/// it under the sender's name; a state the size of a book is refused before
/// it is sent. That the touch is what the cell's book shows is proven where
/// the cell is, `qip-edge-node/tests/edge_quotes.rs`.
///
/// Mutation: remove the `MAX_EDGES_PER_STATE` comparison from
/// `check_edges` and the oversized state is framed and sent.
#[test]
fn a_nodes_executable_edge_state_reaches_its_peer_and_a_book_sized_one_is_refused() -> Result<()> {
    let (mut americas, mut europe) = pair("americas", "europe")?;
    let mut touch = edge(0);
    touch.instrument = "obj-ACME".into();
    touch.bid_size = dec!("500");
    touch.ask_size = dec!("400");
    let turn = americas.turn(vec![touch], at(3))?;
    assert_eq!(turn.edges_published, 1, "premise: one book's touch");
    assert!(turn.offered);
    deliver(&mut europe, at(4));
    let held = europe
        .endpoint()
        .peer_edges("americas")
        .expect("the peer holds the sender's edge state");
    assert_eq!(held.len(), 1);
    assert_eq!(
        (held[0].bid, held[0].bid_size, held[0].ask, held[0].ask_size),
        (dec!("99"), dec!("500"), dec!("101"), dec!("400")),
        "the edge is the touch the sender published"
    );

    // A state with more edges than a summary may carry is refused at the
    // sender, naming the bound.
    let bounded: Vec<ExecutableEdge> = (0..MAX_EDGES_PER_STATE).map(edge).collect();
    assert!(
        americas.endpoint().edge_state_frame(2, bounded).is_ok(),
        "premise: a state at the bound is sent"
    );
    let oversized: Vec<ExecutableEdge> = (0..=MAX_EDGES_PER_STATE).map(edge).collect();
    let refused = americas
        .endpoint()
        .edge_state_frame(3, oversized)
        .expect_err("a book-sized state must be refused");
    assert!(
        refused.message().contains("never its book"),
        "{}",
        refused.message()
    );
    Ok(())
}

/// A replayed or reordered edge state does not roll a peer's view back.
///
/// Mutation: drop the `held >= sequence` comparison from
/// `PeerEndpoint::absorb_edges` and the stale state overwrites the newer.
#[test]
fn an_older_edge_state_does_not_replace_a_newer_one() -> Result<()> {
    let (americas, mut europe) = pair("americas", "europe")?;
    let newer = americas.endpoint().edge_state_frame(5, vec![edge(1)])?;
    let older = americas.endpoint().edge_state_frame(4, vec![edge(2)])?;
    assert!(americas.send(newer));
    deliver(&mut europe, at(1));
    assert!(americas.send(older));
    deliver(&mut europe, at(2));
    let held = europe.endpoint().peer_edges("americas").expect("held");
    assert_eq!(
        held[0].instrument, "obj-1",
        "the older state replaced the newer"
    );
    assert!(
        europe
            .endpoint()
            .refusals()
            .iter()
            .any(|refusal| refusal.kind == "stale_edge_state")
    );
    Ok(())
}

/// The production path, end to end through the port alone (ADR 0116 §3): a
/// quote the decision loop publishes reaches a peer as edge state, framed on
/// the session thread; a crossed quote is refused there and counted, never
/// sent; and the status the port copies reports both.
///
/// Mutation (run): make `host::edges` keep a quote whose `validate` fails —
/// the endpoint then refuses to frame the whole state, so the tradeable
/// touch never crosses either and the wait times out with
/// `publish_failures: 1, quotes_refused: 0`. Mutation: make the
/// session thread `listen` instead of `turn` when an offer arrives, and no
/// edge state ever reaches the peer, so the wait times out.
#[test]
fn a_quote_published_through_the_port_reaches_a_peer_and_a_crossed_one_is_refused_on_the_session_thread()
-> Result<()> {
    let (port_at, peer_at) = (free_port(), free_port());
    let (port, bound) = qip_peer_link::spawn(
        &settings("americas", port_at, "europe", peer_at),
        Arc::new(SystemClock),
    )?;
    assert_eq!(
        bound, port_at,
        "premise: the port's session bound where it was told"
    );
    let mut europe = PeerSession::new(&settings("europe", peer_at, "americas", port_at))?;

    let quote = |instrument: &str, bid: Decimal, ask: Decimal| EdgeQuote {
        venue: "XLON".into(),
        instrument: instrument.into(),
        bid,
        bid_size: dec!("10"),
        ask,
        ask_size: dec!("10"),
        as_of_ns: OPENED,
    };
    assert!(port.try_publish(vec![
        quote("obj-1", dec!("99"), dec!("101")),
        // Crossed: a bid above the ask is not a touch anyone can trade.
        quote("obj-2", dec!("102"), dec!("101")),
    ]));

    let began = std::time::Instant::now();
    let held = loop {
        for frame in europe.link().take_inbound() {
            let _ = europe.endpoint_mut().receive(&frame, at(1));
        }
        if let Some(held) = europe.endpoint().peer_edges("americas") {
            break held.to_vec();
        }
        assert!(
            began.elapsed() < PATIENCE,
            "no edge state crossed from the port within {PATIENCE:?}: {:?}",
            port.status()
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    assert_eq!(held.len(), 1, "only the tradeable touch crossed: {held:?}");
    assert_eq!(held[0].instrument, "obj-1");

    // The status is copied after each session turn, so it trails the wire by
    // up to one turn; wait for the write the peer already saw to show.
    let began = std::time::Instant::now();
    while port.status().sent == 0 {
        assert!(
            began.elapsed() < PATIENCE,
            "the port never reported the write: {:?}",
            port.status()
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let status = port.status();
    assert_eq!(status.quotes_refused, 1, "{status:?}");
    assert_eq!(status.dropped_offers, 0, "{status:?}");
    Ok(())
}
