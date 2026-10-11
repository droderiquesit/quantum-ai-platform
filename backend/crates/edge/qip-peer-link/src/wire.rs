//! The peer Reflex Mesh on a socket: cells talking to cells (R12-MESH-5035,
//! 5046, 5050, 5060).
//!
//! `qip_peer` is the protocol — epochs, reservations, FIRE, fills, unwinds,
//! declines and executable edge states — and it was tested with no wire
//! under it until `215138c2`: latency-sensitive coordination between
//! regions had no path but through the centre, which is the broker-mediated
//! hop §16 says it must not take.
//!
//! This module is the wire, and only the wire: frames in, frames out. Every
//! rule stays in the protocol, so the transport can lose, delay or refuse a
//! frame and cannot admit one the endpoint would refuse.
//!
//! # Off the decision thread
//!
//! A [`PeerLink`] owns a thread that accepts connections, reads frames,
//! connects to peers and writes to them. Its owner — in production the
//! session thread `crate::host` starts, never the reflex cell's decision
//! thread (ADR 0116) — meets it at two bounded channels and never waits on
//! either: [`PeerLink::offer`] is a `try_send` and a full queue is a counted
//! drop; [`PeerLink::take_inbound`] is `try_recv`. Connecting and writing carry explicit timeouts, and a peer
//! that cannot be reached backs off rather than being retried on every frame.
//! That is what makes a peer outage unable to hold the journal flush or the
//! centre's policy downlink (MESH-5050): neither passes through here, and
//! nothing here can block the thread they run on.
//!
//! # Plaintext, and therefore loopback only
//!
//! ADR 0112 puts the peer mesh under mTLS through rustls. This build does not
//! carry rustls, so the transport speaks the protocol's length-prefixed
//! frames in plaintext and [`PeerSettings::read`] refuses any address that is
//! not loopback — a cross-region peer link in plaintext would carry
//! reservations and fills across the internet unauthenticated. Loopback is
//! enough to prove two nodes coordinate over a real socket; the mTLS
//! transport is the remaining step, and it replaces the stream type here and
//! nothing in the protocol.

use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_peer::{ExecutableEdge, MAX_EDGE_PEERS, PeerEndpoint, PeerLimits, Refusal};
use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};

/// The address this node accepts peer connections on.
pub const LISTEN_VARIABLE: &str = "QIP_PEER_MESH_LISTEN";
/// The peers: `name=host:port,name=host:port`.
pub const PEERS_VARIABLE: &str = "QIP_PEER_MESH_PEERS";

/// The largest frame either side sends or accepts.
pub const MAX_FRAME: usize = 64 * 1024;
/// How long a connect may take before the peer is treated as down.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(200);
/// How long one frame's write may take.
const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(200);
/// How long a peer that could not be reached is left alone.
const BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);
/// How long the worker sleeps when there was nothing to do.
const IDLE: std::time::Duration = std::time::Duration::from_millis(2);
/// Frames queued in each direction between the decision thread and the wire.
const QUEUE: usize = 256;
/// Inbound connections held at once, beyond which a new one is closed.
const MAX_INBOUND: usize = MAX_EDGE_PEERS;

/// Where this node listens and who its peers are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerSettings {
    pub node: String,
    pub listen: SocketAddr,
    pub peers: BTreeMap<String, SocketAddr>,
}

impl PeerSettings {
    /// `None` when neither variable is set. Refuses one without the other,
    /// a malformed entry, a peer named as this node, and — until the mTLS
    /// transport of ADR 0112 exists — any address that is not loopback.
    pub fn read(node: &str, listen: Option<&str>, peers: Option<&str>) -> Result<Option<Self>> {
        fn present(value: Option<&str>) -> Option<&str> {
            value.map(str::trim).filter(|v| !v.is_empty())
        }
        let (listen, peers) = match (present(listen), present(peers)) {
            (None, None) => return Ok(None),
            (Some(listen), Some(peers)) => (listen, peers),
            _ => {
                return Err(Error::invalid(format!(
                    "configuration: set both {LISTEN_VARIABLE} and {PEERS_VARIABLE}, or neither; \
                     a peer mesh that only listens or only speaks coordinates nothing"
                )));
            }
        };
        let listen = loopback(LISTEN_VARIABLE, listen)?;
        let mut parsed = BTreeMap::new();
        for entry in peers.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let (name, address) = entry.split_once('=').ok_or_else(|| {
                Error::invalid(format!(
                    "configuration: {PEERS_VARIABLE} entry `{entry}` is not `name=host:port`"
                ))
            })?;
            let name = name.trim();
            if name.is_empty() || name == node {
                return Err(Error::invalid(format!(
                    "configuration: {PEERS_VARIABLE} entry `{entry}` names no peer or names this \
                     node; a node is not its own peer"
                )));
            }
            if parsed
                .insert(name.to_string(), loopback(PEERS_VARIABLE, address)?)
                .is_some()
            {
                return Err(Error::invalid(format!(
                    "configuration: {PEERS_VARIABLE} names `{name}` twice"
                )));
            }
        }
        if parsed.is_empty() || parsed.len() > MAX_EDGE_PEERS {
            return Err(Error::invalid(format!(
                "configuration: {PEERS_VARIABLE} names {} peers; between one and \
                 {MAX_EDGE_PEERS}",
                parsed.len()
            )));
        }
        Ok(Some(Self {
            node: node.to_string(),
            listen,
            peers: parsed,
        }))
    }
}

fn loopback(variable: &str, value: &str) -> Result<SocketAddr> {
    let address: SocketAddr = value.trim().parse().map_err(|_| {
        Error::invalid(format!(
            "configuration: {variable}: `{value}` is not an ip:port address"
        ))
    })?;
    if !address.ip().is_loopback() {
        return Err(Error::invalid(format!(
            "configuration: {variable}: `{value}` is not loopback. The peer mesh crosses hosts \
             only under mTLS (ADR 0112), which this build does not carry; plaintext frames \
             carrying reservations and fills are admitted on loopback alone"
        )));
    }
    Ok(address)
}

/// What the wire has done, for the health body and the tests.
#[derive(Debug, Default)]
pub struct PeerCounters {
    sent: AtomicU64,
    send_failures: AtomicU64,
    queued_drops: AtomicU64,
    received: AtomicU64,
    inbound_drops: AtomicU64,
    oversized: AtomicU64,
}

/// A copy of [`PeerCounters`] at one instant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeerWireStats {
    /// Frame copies written to a peer's socket.
    pub sent: u64,
    /// Writes or connects that failed; the peer then backs off.
    pub send_failures: u64,
    /// Frames the decision thread offered while the outbound queue was full.
    pub queued_drops: u64,
    /// Whole frames read from peers and handed to the decision thread.
    pub received: u64,
    /// Frames read while the inbound queue was full.
    pub inbound_drops: u64,
    /// Connections closed for declaring a frame over [`MAX_FRAME`].
    pub oversized: u64,
}

impl PeerCounters {
    fn snapshot(&self) -> PeerWireStats {
        PeerWireStats {
            sent: self.sent.load(Ordering::Relaxed),
            send_failures: self.send_failures.load(Ordering::Relaxed),
            queued_drops: self.queued_drops.load(Ordering::Relaxed),
            received: self.received.load(Ordering::Relaxed),
            inbound_drops: self.inbound_drops.load(Ordering::Relaxed),
            oversized: self.oversized.load(Ordering::Relaxed),
        }
    }
}

/// The session's end of the wire.
#[derive(Debug)]
pub struct PeerLink {
    local: SocketAddr,
    outbound: SyncSender<Vec<u8>>,
    inbound: Receiver<Vec<u8>>,
    counters: Arc<PeerCounters>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl PeerLink {
    /// Bind `settings.listen` and start the wire's thread.
    pub fn spawn(settings: &PeerSettings) -> Result<Self> {
        let listener = TcpListener::bind(settings.listen).map_err(|error| {
            Error::io(format!(
                "the peer mesh cannot listen on {}: {error}",
                settings.listen
            ))
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|error| Error::io(format!("the peer listener: {error}")))?;
        let local = listener
            .local_addr()
            .map_err(|error| Error::io(format!("the peer listener: {error}")))?;
        let (outbound, to_wire) = sync_channel::<Vec<u8>>(QUEUE);
        let (from_wire, inbound) = sync_channel::<Vec<u8>>(QUEUE);
        let counters = Arc::new(PeerCounters::default());
        let stop = Arc::new(AtomicBool::new(false));
        let mut wire = Wire {
            listener,
            peers: settings
                .peers
                .iter()
                .map(|(name, address)| {
                    (
                        name.clone(),
                        Outbound {
                            address: *address,
                            stream: None,
                            retry_at: None,
                        },
                    )
                })
                .collect(),
            inbound: Vec::new(),
            to_wire,
            from_wire,
            counters: Arc::clone(&counters),
            stop: Arc::clone(&stop),
        };
        let worker = std::thread::Builder::new()
            .name("qip-peer-mesh".to_string())
            .spawn(move || wire.run())
            .map_err(|error| Error::io(format!("cannot start the peer mesh: {error}")))?;
        Ok(Self {
            local,
            outbound,
            inbound,
            counters,
            stop,
            worker: Some(worker),
        })
    }

    /// The address actually bound, which differs from the configured one
    /// when the configuration asked for port zero.
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// Offer one frame to every peer. Never waits: `false` is a full queue
    /// or a stopped wire, counted and dropped.
    pub fn offer(&self, frame: Vec<u8>) -> bool {
        match self.outbound.try_send(frame) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.counters.queued_drops.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Every whole frame read since the last call, oldest first. Never waits.
    pub fn take_inbound(&self) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        while frames.len() < QUEUE {
            match self.inbound.try_recv() {
                Ok(frame) => frames.push(frame),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        frames
    }

    pub fn stats(&self) -> PeerWireStats {
        self.counters.snapshot()
    }
}

impl Drop for PeerLink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Debug)]
struct Outbound {
    address: SocketAddr,
    stream: Option<TcpStream>,
    retry_at: Option<std::time::Instant>,
}

#[derive(Debug)]
struct Inbound {
    stream: TcpStream,
    buffer: Vec<u8>,
    open: bool,
}

struct Wire {
    listener: TcpListener,
    peers: BTreeMap<String, Outbound>,
    inbound: Vec<Inbound>,
    to_wire: Receiver<Vec<u8>>,
    from_wire: SyncSender<Vec<u8>>,
    counters: Arc<PeerCounters>,
    stop: Arc<AtomicBool>,
}

impl Wire {
    fn run(&mut self) {
        while !self.stop.load(Ordering::Relaxed) {
            let accepted = self.accept();
            let read = self.read();
            let sent = match self.send() {
                Some(sent) => sent,
                // The decision thread's end is gone: nothing more to carry.
                None => return,
            };
            if !(accepted || read || sent) {
                std::thread::sleep(IDLE);
            }
        }
    }

    fn accept(&mut self) -> bool {
        let mut any = false;
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    any = true;
                    if self.inbound.len() >= MAX_INBOUND || stream.set_nonblocking(true).is_err() {
                        continue;
                    }
                    self.inbound.push(Inbound {
                        stream,
                        buffer: Vec::new(),
                        open: true,
                    });
                }
                Err(_) => return any,
            }
        }
    }

    fn read(&mut self) -> bool {
        let mut any = false;
        let mut chunk = [0u8; 8 * 1024];
        for connection in &mut self.inbound {
            loop {
                match connection.stream.read(&mut chunk) {
                    Ok(0) => {
                        connection.open = false;
                        break;
                    }
                    Ok(read) => {
                        any = true;
                        connection.buffer.extend_from_slice(&chunk[..read]);
                        // Bounded: a connection holds at most one frame and
                        // its prefix beyond what has been parsed.
                        if connection.buffer.len() > 2 * (MAX_FRAME + 4) {
                            connection.open = false;
                            break;
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == ErrorKind::Interrupted => {}
                    Err(_) => {
                        connection.open = false;
                        break;
                    }
                }
            }
            while connection.buffer.len() >= 4 {
                let mut head = [0u8; 4];
                head.copy_from_slice(&connection.buffer[..4]);
                let declared = u32::from_be_bytes(head) as usize;
                if declared > MAX_FRAME {
                    // Refused before a byte of it is buffered as a frame.
                    self.counters.oversized.fetch_add(1, Ordering::Relaxed);
                    connection.open = false;
                    connection.buffer.clear();
                    break;
                }
                if connection.buffer.len() < 4 + declared {
                    break;
                }
                let frame: Vec<u8> = connection.buffer.drain(..4 + declared).collect();
                match self.from_wire.try_send(frame) {
                    Ok(()) => {
                        self.counters.received.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(TrySendError::Full(_)) => {
                        self.counters.inbound_drops.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        connection.open = false;
                        break;
                    }
                }
            }
        }
        self.inbound.retain(|connection| connection.open);
        any
    }

    /// `None` once the decision thread has dropped its end.
    fn send(&mut self) -> Option<bool> {
        let mut any = false;
        loop {
            let frame = match self.to_wire.try_recv() {
                Ok(frame) => frame,
                Err(TryRecvError::Empty) => return Some(any),
                Err(TryRecvError::Disconnected) => return None,
            };
            any = true;
            let now = std::time::Instant::now();
            for peer in self.peers.values_mut() {
                if peer.stream.is_none() {
                    if peer.retry_at.is_some_and(|at| now < at) {
                        self.counters.send_failures.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    match TcpStream::connect_timeout(&peer.address, CONNECT_TIMEOUT).and_then(
                        |stream| {
                            stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
                            stream.set_nodelay(true)?;
                            Ok(stream)
                        },
                    ) {
                        Ok(stream) => {
                            peer.stream = Some(stream);
                            peer.retry_at = None;
                        }
                        Err(_) => {
                            peer.retry_at = Some(now + BACKOFF);
                            self.counters.send_failures.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                    }
                }
                let written = peer
                    .stream
                    .as_mut()
                    .map(|stream| stream.write_all(&frame).and_then(|()| stream.flush()));
                match written {
                    Some(Ok(())) => {
                        self.counters.sent.fetch_add(1, Ordering::Relaxed);
                    }
                    _ => {
                        peer.stream = None;
                        peer.retry_at = Some(now + BACKOFF);
                        self.counters.send_failures.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }
}

/// The limits this node's endpoint enforces.
pub fn node_limits() -> PeerLimits {
    PeerLimits {
        max_frame: MAX_FRAME,
        ttl_ceiling: Duration::from_secs(30),
        // A node joins a distributed epoch only with a measured clock; this
        // build measures none, so the endpoint refuses to join one by name
        // ("clock_unknown") rather than assuming an offset of zero.
        max_clock_offset: Duration::from_millis(5),
        max_fan_out: MAX_EDGE_PEERS,
    }
}

/// What one turn of the peer session did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeerTurn {
    /// Frames read from the wire this turn.
    pub received: usize,
    /// Of those, frames the endpoint refused (each is in its refusals).
    pub refused: usize,
    /// Edges in this node's state, and whether the wire took the frame.
    pub edges_published: usize,
    pub offered: bool,
}

/// The protocol endpoint and the wire, driven together.
///
/// Public for Lane 1 callers and for tests that drive two sessions by hand.
/// The reflex node never holds one: in production `crate::spawn` moves it
/// onto a session thread of its own, so receiving, refusing, framing edge
/// state and expiring epochs never run on the thread that runs `Cell::work`
/// (ADR 0116 §2). Before that ADR this ran on the decision thread and took
/// the cell itself, which made coordination work part of the reflex budget.
#[derive(Debug)]
pub struct PeerSession {
    endpoint: PeerEndpoint,
    link: PeerLink,
    sequence: u64,
}

impl PeerSession {
    pub fn new(settings: &PeerSettings) -> Result<Self> {
        Ok(Self {
            endpoint: PeerEndpoint::new(&settings.node, node_limits()),
            link: PeerLink::spawn(settings)?,
            sequence: 0,
        })
    }

    pub fn endpoint(&self) -> &PeerEndpoint {
        &self.endpoint
    }

    pub fn endpoint_mut(&mut self) -> &mut PeerEndpoint {
        &mut self.endpoint
    }

    pub fn link(&self) -> &PeerLink {
        &self.link
    }

    /// Offer a protocol frame — a token, a reservation, FIRE — to the peers.
    pub fn send(&self, frame: Vec<u8>) -> bool {
        self.link.offer(frame)
    }

    /// Apply what peers sent, then publish this node's executable edge state,
    /// then expire what has lapsed. Nothing here waits: the inbound queue is
    /// drained with `try_recv` and the state is offered with `try_send`.
    ///
    /// Takes the edges, not the cell: the session cannot see the book, so a
    /// summary is all it can send (MESH-5060), and it holds nothing a
    /// reflex crate could hand it by reference and then wait on.
    pub fn turn(&mut self, edges: Vec<ExecutableEdge>, now: Timestamp) -> Result<PeerTurn> {
        let mut turn = self.absorb(now);
        turn.edges_published = edges.len();
        self.sequence = self.sequence.saturating_add(1);
        let frame = self.endpoint.edge_state_frame(self.sequence, edges)?;
        turn.offered = self.link.offer(frame);
        self.endpoint.forget_expired(now);
        Ok(turn)
    }

    /// Apply what peers sent and expire what has lapsed, publishing nothing.
    /// The session thread runs this when the cell has offered no fresh
    /// quotes, so an epoch still expires on time and a peer's frames are
    /// still read while this cell is quiet.
    pub fn listen(&mut self, now: Timestamp) -> PeerTurn {
        let turn = self.absorb(now);
        self.endpoint.forget_expired(now);
        turn
    }

    fn absorb(&mut self, now: Timestamp) -> PeerTurn {
        let mut turn = PeerTurn::default();
        for frame in self.link.take_inbound() {
            turn.received += 1;
            if self.endpoint.receive(&frame, now).is_err() {
                turn.refused += 1;
            }
        }
        turn
    }

    /// The endpoint's refusals, for the log.
    pub fn refusals(&self) -> &[Refusal] {
        self.endpoint.refusals()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_peer_address_off_loopback_is_refused_until_the_mtls_transport_exists() {
        let error = PeerSettings::read("a", Some("127.0.0.1:0"), Some("b=10.0.0.2:7000"))
            .expect_err("a non-loopback peer must be refused");
        assert!(error.message().contains("ADR 0112"), "{}", error.message());
        assert!(
            PeerSettings::read("a", Some("0.0.0.0:7000"), Some("b=127.0.0.1:7001")).is_err(),
            "listening on every interface is not loopback"
        );
        assert!(
            PeerSettings::read("a", None, None)
                .expect("unset")
                .is_none()
        );
        assert!(PeerSettings::read("a", Some("127.0.0.1:0"), None).is_err());
        assert!(PeerSettings::read("a", Some("127.0.0.1:0"), Some("a=127.0.0.1:1")).is_err());
        let settings = PeerSettings::read("a", Some("127.0.0.1:0"), Some("b=127.0.0.1:7001"))
            .expect("well formed")
            .expect("configured");
        assert_eq!(settings.peers.len(), 1);
    }
}
