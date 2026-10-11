//! The peer mesh off-host: mutual TLS 1.3 with workload certificates
//! (ADR 0112, R12-MESH-5035).
//!
//! Before this the link carried plaintext frames and so accepted loopback
//! addresses alone, which meant no cross-region link could be configured at
//! all. These tests drive two real sessions over real sockets under TLS and
//! prove the three things a peer link off-host must hold: a frame from an
//! admitted peer arrives tagged with the name its certificate proved; a
//! certificate from the right authority naming a workload this node does not
//! peer with is refused at the handshake; and a peer that speaks plaintext
//! to the TLS listener delivers nothing.

#![allow(clippy::panic_in_result_fn)]
#![allow(clippy::unwrap_used, clippy::expect_used)] // integration tests may unwrap: a panic is the failure report

#[path = "../../../libs/qip-transport/tests/support/certs.rs"]
mod certs;

use qip_core::SystemClock;
use qip_core::error::Result;
use qip_peer::PeerMessage;
use qip_peer_link::{DEFAULT_CLOCK_BOUND, MAX_FRAME, PeerSession, PeerSettings};
use qip_transport::tls::{LinkSecurity, WorkloadIdentity};
use std::collections::BTreeMap;
use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;

const PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

fn free_port() -> SocketAddr {
    let probe = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    probe.local_addr().expect("its address")
}

fn security(leaf: &certs::Minted, ca: &certs::Minted) -> LinkSecurity {
    LinkSecurity::MutualTls(Arc::new(
        WorkloadIdentity::from_pem(
            leaf.certificate_pem().as_bytes(),
            leaf.key_pem().as_bytes(),
            ca.certificate_pem().as_bytes(),
        )
        .expect("minted material loads"),
    ))
}

fn session(
    node: &str,
    listen: SocketAddr,
    peer: &str,
    peer_at: SocketAddr,
    security: LinkSecurity,
) -> Result<PeerSession> {
    PeerSession::with_clock(
        &PeerSettings {
            node: node.into(),
            listen,
            peers: BTreeMap::from([(peer.to_string(), peer_at)]),
            security,
            clock_bound: DEFAULT_CLOCK_BOUND,
        },
        Arc::new(SystemClock),
    )
}

fn edge_state(node: &str) -> Vec<u8> {
    qip_peer::encode(
        &PeerMessage::EdgeState {
            node: node.into(),
            sequence: 1,
            edges: vec![],
        },
        MAX_FRAME,
    )
    .expect("an empty edge state encodes")
}

/// Mutation: make `Wire::read` skip the `PeerNames::admit` step (accept any
/// certificate the roots chain) and the unlisted workload's frame arrives;
/// make the listener plaintext (`tls: None`) and the plaintext writer's
/// frame arrives.
#[test]
fn a_peer_link_under_mutual_tls_admits_only_the_configured_peers_and_tags_what_they_send()
-> Result<()> {
    let ca = certs::authority("qip peer mesh test pool");
    let americas = certs::leaf(&ca, 2, "americas");
    let europe = certs::leaf(&ca, 3, "europe");
    let tokyo = certs::leaf(&ca, 4, "tokyo");

    let (americas_at, europe_at, tokyo_at) = (free_port(), free_port(), free_port());
    let sender = session(
        "americas",
        americas_at,
        "europe",
        europe_at,
        security(&americas, &ca),
    )?;
    let receiver = session(
        "europe",
        europe_at,
        "americas",
        americas_at,
        security(&europe, &ca),
    )?;

    // An admitted peer: the frame crosses, tagged with the proven name.
    assert!(sender.send(edge_state("americas")));
    let began = std::time::Instant::now();
    let arrived = loop {
        let frames = receiver.link().take_inbound();
        if let Some(frame) = frames.into_iter().next() {
            break frame;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "no frame crossed the TLS link: sender {:?}, receiver {:?}",
            sender.link().stats(),
            receiver.link().stats()
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    };
    assert_eq!(arrived.from.as_deref(), Some("americas"));
    assert_eq!(arrived.frame, edge_state("americas"));

    // A certificate from the same authority, for a workload europe does not
    // peer with: the handshake completes on the chain and is refused on the
    // name, and nothing it sends arrives.
    let stranger = session(
        "tokyo",
        tokyo_at,
        "europe",
        europe_at,
        security(&tokyo, &ca),
    )?;
    assert!(stranger.send(edge_state("tokyo")));
    // Plaintext at the TLS listener: no handshake, no frame.
    let mut plain = TcpStream::connect(europe_at).expect("the listener accepts the connection");
    plain
        .write_all(&edge_state("americas"))
        .expect("the bytes are written");

    let began = std::time::Instant::now();
    while receiver.link().stats().handshakes_refused < 2 {
        assert!(
            began.elapsed() < PATIENCE,
            "the listener did not refuse both: {:?}; the stranger's wire {:?}",
            receiver.link().stats(),
            stranger.link().stats()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        receiver.link().take_inbound().is_empty(),
        "a refused connection delivered a frame"
    );
    assert_eq!(receiver.link().stats().received, 1, "only americas' frame");
    drop(plain);
    Ok(())
}

/// Under mutual TLS a peer may be off loopback — that is the point — and
/// under plaintext it may not, whatever the settings were built from.
///
/// Mutation: drop the loopback loop from `PeerSettings::validate` and the
/// plaintext spawn binds.
#[test]
fn an_off_host_peer_is_admitted_under_mutual_tls_and_refused_in_plaintext_at_spawn() {
    let ca = certs::authority("qip peer mesh test pool");
    let leaf = certs::leaf(&ca, 2, "americas");
    let off_host: SocketAddr = "10.0.0.2:7000".parse().expect("an address");
    let mut settings = PeerSettings {
        node: "americas".into(),
        listen: free_port(),
        peers: BTreeMap::from([("europe".to_string(), off_host)]),
        security: security(&leaf, &ca),
        clock_bound: DEFAULT_CLOCK_BOUND,
    };
    assert!(
        settings.validate().is_ok(),
        "mutual TLS admits an off-host peer"
    );
    settings.security = LinkSecurity::plaintext_for_tests();
    let refused =
        PeerSession::new(&settings).expect_err("a plaintext off-host peer must not spawn");
    assert!(
        refused.message().contains("not loopback"),
        "{}",
        refused.message()
    );
}
