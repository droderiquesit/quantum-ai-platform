//! The peer Reflex Mesh between two nodes, over the socket each node's
//! `main.rs` spawns (R12-MESH-1010).
//!
//! `qip-peer-link`'s own tests carry a cycle between two `PeerSession`s the
//! test turns by hand. What they cannot show is the node's composition: two
//! `qip_peer_link::spawn` sessions on their own threads, each fed only what a
//! decision loop publishes through `ReflexPort`, originating, reserving and
//! refusing on their own clocks. That is the exchange a cell has with a peer,
//! and this file holds it.
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

use qip_core::{Clock, Decimal, SystemClock, dec};
use qip_peer_link::boundary::{EdgeQuote, LegAuthority, Offer, PeerStatus, ReflexPort, Side};
use qip_peer_link::{DEFAULT_CLOCK_BOUND, PeerSettings};
use qip_transport::tls::LinkSecurity;
use std::collections::BTreeMap;
use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

const COORDINATOR: &str = "london-1";
const PARTICIPANT: &str = "tokyo-1";
const INSTRUMENT: &str = "obj-ACME";
/// Clock probes run every 250 ms and a first connect that finds no listener
/// backs off for a second; a saturated workspace run shares the cores.
const PATIENCE: Duration = Duration::from_secs(60);

fn free() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|probe| probe.local_addr())
        .expect("a loopback port")
}

fn spawn(node: &str, at: SocketAddr, peer: &str, peer_at: SocketAddr) -> Option<ReflexPort> {
    qip_peer_link::spawn(
        &PeerSettings {
            node: node.to_string(),
            listen: at,
            peers: BTreeMap::from([(peer.to_string(), peer_at)]),
            security: LinkSecurity::plaintext_for_tests(),
            clock_bound: DEFAULT_CLOCK_BOUND,
        },
        Arc::new(SystemClock),
    )
    .ok()
    .map(|(port, _)| port)
}

/// A port taken between the probe and the bind is an `Err` from `spawn`, not
/// a finding; a fresh pair turns that race into a retry.
fn linked_pair() -> (ReflexPort, ReflexPort, SocketAddr) {
    for _ in 0..10 {
        let (london_at, tokyo_at) = (free(), free());
        if let (Some(london), Some(tokyo)) = (
            spawn(COORDINATOR, london_at, PARTICIPANT, tokyo_at),
            spawn(PARTICIPANT, tokyo_at, COORDINATOR, london_at),
        ) {
            return (london, tokyo, tokyo_at);
        }
    }
    panic!("two loopback listeners within ten attempts");
}

fn quote(venue: &str, bid: Decimal, ask: Decimal, now_ns: i64) -> EdgeQuote {
    EdgeQuote {
        venue: venue.to_string(),
        instrument: INSTRUMENT.to_string(),
        bid,
        bid_size: dec!("50"),
        ask,
        ask_size: dec!("50"),
        as_of_ns: now_ns,
    }
}

/// `spread` is the book's own: what crossing back out of the leg costs per
/// unit, which the epoch's declared unwind loss must cover or the leg is
/// declined.
fn authority(
    venue: &str,
    side: Side,
    limit: Decimal,
    spread: Decimal,
    now_ns: i64,
) -> LegAuthority {
    LegAuthority {
        venue: venue.to_string(),
        instrument: INSTRUMENT.to_string(),
        side,
        limit_price: limit,
        max_size: dec!("50"),
        touch_size: dec!("50"),
        unwind_per_unit: spread,
        valid_until_ns: now_ns + 60_000_000_000,
    }
}

/// london buys at its ask of 101 and tokyo sells at its bid of 103: two
/// hundred basis points, far past the session's minimum edge. Each node
/// authorises its own leg of that crossing and nothing else.
fn offers(now_ns: i64) -> (Offer, Offer) {
    let london = Offer {
        quotes: vec![quote("XLON", dec!("99"), dec!("101"), now_ns)],
        legs: vec![authority("XLON", Side::Buy, dec!("101"), dec!("2"), now_ns)],
        withheld: 0,
    };
    let tokyo = Offer {
        quotes: vec![quote("XTKS", dec!("103"), dec!("104"), now_ns)],
        legs: vec![authority(
            "XTKS",
            Side::Sell,
            dec!("103"),
            dec!("1"),
            now_ns,
        )],
        withheld: 0,
    };
    (london, tokyo)
}

/// A token frame written by hand, as a peer that does not use `qip-peer`'s
/// builder would write it: the protocol's four-byte length prefix and its
/// JSON, with `legs` legs alternating between london, which coordinates, and
/// tokyo.
fn token_frame(opportunity: &str, legs: usize, now_ns: i64) -> Vec<u8> {
    let legs: Vec<serde_json::Value> = (0..legs)
        .map(|index| {
            // Every leg on one of the two linked nodes, alternately. A leg on
            // a node tokyo has no clock for is refused by the clock bound
            // before the leg bound is read, and then the refusal below would
            // prove the wrong rule.
            let node = if index % 2 == 0 {
                COORDINATOR
            } else {
                PARTICIPANT
            };
            serde_json::json!({
                "leg": format!("leg-{index}"),
                "node": node,
                "size": "1",
                "unwind_loss": "1",
            })
        })
        .collect();
    let body = serde_json::to_vec(&serde_json::json!({
        "kind": "token",
        "epoch": {
            "opportunity": opportunity,
            "sequence": 1,
            "cycle": "obj-ACME:XLON>XTKS",
            "legs": legs,
            "size": "1",
            "min_edge_bps": "5",
            "model_versions": ["hand-written"],
            "opened_at_nanos": now_ns,
            "ttl_nanos": 200_000_000,
            "max_recovery_loss": "100",
        }
    }))
    .expect("a JSON value serialises");
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend_from_slice(&body);
    frame
}

/// Write one frame to `at` on a fresh plaintext connection, as a peer would,
/// and hand back the connection: the wire drops what a connection buffered
/// once it reads the close, so a writer that hangs up at once may never have
/// been read at all, and a refusal of that is no refusal.
fn send_raw(at: SocketAddr, frame: &[u8]) -> TcpStream {
    let mut stream = TcpStream::connect(at).expect("the participant's listener");
    stream.write_all(frame).expect("the frame is written");
    stream.flush().expect("the frame is flushed");
    stream
}

fn wait_for(what: &str, mut until: impl FnMut() -> bool, mut each: impl FnMut()) {
    let began = Instant::now();
    while !until() {
        assert!(began.elapsed() < PATIENCE, "timed out waiting for {what}");
        each();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// R12-MESH-1010's node half. Two spawned sessions, each fed only the touch
/// and the one leg its cell authorises, find the crossing between them; the
/// coordinator opens the epoch, each node reserves **its own** leg, and the
/// coordinator, having seen the participant's reservation, declares FIRE —
/// which it can do only once every leg is reserved. Then a token of
/// twenty-one legs, written by hand and sent to the participant's socket, is
/// refused there and opens nothing, though the same hand-written token at two
/// legs is decoded and judged.
///
/// Mutation (run): skip the participant's `Reserved` send in the saga, and
/// the coordinator never sees a reservation and never fires; the wait times
/// out. Mutation (run): skip the epoch's `validate` in
/// `PeerEndpoint::install`, and the twenty-one-leg token is installed and
/// judged at the participant like the two-leg one; this fails.
#[test]
fn two_nodes_each_reserve_their_own_leg_of_one_epoch_over_the_peer_socket_and_a_twenty_one_leg_token_opens_nothing()
 {
    let (london, tokyo, tokyo_at) = linked_pair();
    let clock = SystemClock;
    let publish = || {
        let now_ns = clock.now().as_nanos();
        let (london_offer, tokyo_offer) = offers(now_ns);
        london.try_publish(london_offer);
        tokyo.try_publish(tokyo_offer);
    };

    // Premise: nothing has been opened, reserved or fired before either node
    // has published, so every count below is the exchange's.
    let (before_london, before_tokyo) = (london.status(), tokyo.status());
    assert_eq!(
        before_london.cycles_originated + before_london.reservations,
        0
    );
    assert_eq!(before_tokyo.reservations + before_tokyo.declines, 0);

    wait_for(
        "the coordinator to fire on both reservations",
        || london.status().fires >= 1,
        publish,
    );
    let (coordinator, participant) = (london.status(), tokyo.status());
    assert!(
        coordinator.cycles_originated >= 1,
        "london fired a cycle it never opened: {coordinator:?}"
    );
    assert!(
        coordinator.reservations >= 1,
        "the coordinator fired without reserving its own leg: {coordinator:?}"
    );
    assert!(
        participant.reservations >= 1,
        "the coordinator fired with no reservation from tokyo: {participant:?}"
    );
    assert_eq!(
        tokyo.status().cycles_originated,
        0,
        "tokyo sorts after london and coordinates nothing"
    );

    // The leg bound at the receiver. First the premise: a hand-written token
    // of two legs is decoded and judged at tokyo — declined, since its leg
    // names no execution tokyo authorised — so the frame format is right and
    // a refusal of the long one below is about its legs and nothing else.
    //
    // The two cells stop authorising legs first, so no cycle of their own is
    // judged at tokyo while the hand-written ones are: each publishes its
    // touch alone, and the saga holds no authority to originate on.
    let judged = |status: &PeerStatus| status.reservations + status.declines;
    let now_ns = clock.now().as_nanos();
    let (mut london_offer, mut tokyo_offer) = offers(now_ns);
    london_offer.legs.clear();
    tokyo_offer.legs.clear();
    london.try_publish(london_offer);
    tokyo.try_publish(tokyo_offer);
    let mut quiet = (london.status().cycles_originated, judged(&tokyo.status()));
    let mut steady_since = Instant::now();
    let began = Instant::now();
    while steady_since.elapsed() < Duration::from_secs(2) {
        assert!(
            began.elapsed() < PATIENCE,
            "the cells' own cycles never stopped"
        );
        std::thread::sleep(Duration::from_millis(10));
        let now = (london.status().cycles_originated, judged(&tokyo.status()));
        if now != quiet {
            quiet = now;
            steady_since = Instant::now();
        }
    }
    let before = tokyo.status();
    let _two = send_raw(
        tokyo_at,
        &token_frame("hand-two", 2, clock.now().as_nanos()),
    );
    wait_for(
        "the two-leg hand-written token to be judged",
        || judged(&tokyo.status()) > judged(&before),
        || {},
    );

    let before = tokyo.status();
    let _twenty_one = send_raw(
        tokyo_at,
        &token_frame("hand-twenty-one", 21, clock.now().as_nanos()),
    );
    wait_for(
        "the twenty-one-leg token to be refused",
        || tokyo.status().frames_refused > before.frames_refused,
        || {},
    );
    // A turn or two more, so a token that had been installed would have
    // reached tokyo's gate and been judged.
    let settled = tokyo.status().turns + 3;
    wait_for("tokyo to turn", || tokyo.status().turns >= settled, || {});
    let after = tokyo.status();
    assert_eq!(
        judged(&after),
        judged(&before),
        "a twenty-one-leg token reached tokyo's gate: {after:?}"
    );
    assert!(
        after.refusals > before.refusals,
        "the refusal was not journaled: {after:?}"
    );
}
