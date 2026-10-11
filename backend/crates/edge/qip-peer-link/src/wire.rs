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
//! # Mutual TLS off-host, plaintext on loopback alone
//!
//! ADR 0112 puts the peer mesh under mTLS through rustls, and since ADR 0109
//! admitted it this transport speaks the protocol's length-prefixed frames
//! inside TLS 1.3 with a workload certificate on both ends. The listener
//! requires a client certificate chained to the configured roots and then
//! admits only a certificate valid for one of the configured peers' names
//! (`qip_transport::tls::PeerNames`); each outbound connection verifies the
//! peer's certificate for the name it is configured under. Every frame read
//! is tagged with the name its connection authenticated, so the endpoint can
//! refuse a frame that claims to speak for another node
//! (`PeerEndpoint::receive_from`).
//!
//! Plaintext remains for one case: `QIP_DEPLOYMENT=local` with
//! `QIP_TLS_MODE=plaintext`, which `LinkSecurity::configure` alone admits —
//! and even then [`PeerSettings::validate`] refuses any address that is not
//! loopback. A plaintext frame carrying a reservation or a fill across a
//! network is unauthenticated, and no configuration makes that acceptable.
//! A plaintext frame's sender is unknown, so it is checked on content alone.

use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration, SystemClock, Timestamp};
use qip_peer::{ExecutableEdge, MAX_EDGE_PEERS, PeerEndpoint, PeerLimits, Refusal};
use qip_transport::tls::{
    ClientConfig, ClientConnection, LinkSecurity, PeerNames, ServerConfig, ServerConnection,
    ServerName,
};
use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};

/// The address this node accepts peer connections on.
pub const LISTEN_VARIABLE: &str = "QIP_PEER_MESH_LISTEN";
/// The peers: `name=host:port,name=host:port`. Under mutual TLS each name is
/// the DNS name the peer's workload certificate carries.
pub const PEERS_VARIABLE: &str = "QIP_PEER_MESH_PEERS";
/// The widest clock disagreement, in milliseconds, at which this node joins
/// an epoch with a peer: offset plus half the measured round trip.
pub const CLOCK_BOUND_VARIABLE: &str = "QIP_PEER_MESH_CLOCK_BOUND_MS";

/// The clock bound when none is configured. Tight enough for peers in one
/// metro; a cross-region pair measured by probe carries half its round trip
/// as uncertainty, and is refused under this bound until a wider one is
/// stated out loud — see [`PeerSettings::read`].
pub const DEFAULT_CLOCK_BOUND: Duration = Duration::from_millis(5);
/// The widest bound a configuration may state. An epoch's TTL is seconds; a
/// bound past a second would let two regions disagree about whether an epoch
/// is live for a material fraction of it.
pub const MAX_CLOCK_BOUND: Duration = Duration::from_millis(1000);

/// The largest frame either side sends or accepts.
pub const MAX_FRAME: usize = 64 * 1024;
/// How long a connect may take before the peer is treated as down.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(200);
/// How long one frame's write may take.
const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(200);
/// How long a TLS handshake may take, either side. An inbound connection
/// still handshaking past this is closed, so a client that opens and stalls
/// cannot hold one of the bounded inbound slots.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1000);
/// How long a peer that could not be reached is left alone.
const BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);
/// How long the worker sleeps when there was nothing to do.
const IDLE: std::time::Duration = std::time::Duration::from_millis(1);
/// Frames queued in each direction between the session and the wire.
const QUEUE: usize = 256;
/// Inbound connections held at once, beyond which a new one is closed.
const MAX_INBOUND: usize = MAX_EDGE_PEERS;

/// Where this node listens, who its peers are, and how the link is secured.
#[derive(Clone, Debug)]
pub struct PeerSettings {
    pub node: String,
    pub listen: SocketAddr,
    pub peers: BTreeMap<String, SocketAddr>,
    /// Mutual TLS, or plaintext on loopback in a local deployment.
    pub security: LinkSecurity,
    /// See [`CLOCK_BOUND_VARIABLE`].
    pub clock_bound: Duration,
}

impl PeerSettings {
    /// `None` when neither address variable is set. Refuses one without the
    /// other, a malformed entry, a peer named as this node, a clock bound
    /// outside 1..=[`MAX_CLOCK_BOUND`], and whatever [`Self::validate`]
    /// refuses.
    ///
    /// `security` is asked only when the mesh is configured, so a node with
    /// no peers needs no certificate for a link it does not open.
    pub fn read(
        node: &str,
        listen: Option<&str>,
        peers: Option<&str>,
        clock_bound: Option<&str>,
        security: &dyn Fn() -> Result<LinkSecurity>,
    ) -> Result<Option<Self>> {
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
        let listen = address(LISTEN_VARIABLE, listen)?;
        let mut parsed = BTreeMap::new();
        for entry in peers.split(',').map(str::trim).filter(|e| !e.is_empty()) {
            let (name, at) = entry.split_once('=').ok_or_else(|| {
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
                .insert(name.to_string(), address(PEERS_VARIABLE, at)?)
                .is_some()
            {
                return Err(Error::invalid(format!(
                    "configuration: {PEERS_VARIABLE} names `{name}` twice"
                )));
            }
        }
        let clock_bound = match present(clock_bound) {
            None => DEFAULT_CLOCK_BOUND,
            Some(value) => {
                let millis: i64 = value.parse().map_err(|_| {
                    Error::invalid(format!(
                        "configuration: {CLOCK_BOUND_VARIABLE}=`{value}` is not a whole number of \
                         milliseconds"
                    ))
                })?;
                if !(1..=MAX_CLOCK_BOUND.as_millis()).contains(&millis) {
                    return Err(Error::invalid(format!(
                        "configuration: {CLOCK_BOUND_VARIABLE}={millis} is outside 1..={}; a \
                         bound of zero joins nothing and a wider one lets regions disagree about \
                         whether an epoch is live",
                        MAX_CLOCK_BOUND.as_millis()
                    )));
                }
                Duration::from_millis(millis)
            }
        };
        let settings = Self {
            node: node.to_string(),
            listen,
            peers: parsed,
            security: security()
                .map_err(|e| Error::invalid(format!("configuration: {}", e.message())))?,
            clock_bound,
        };
        settings.validate()?;
        Ok(Some(settings))
    }

    /// The checks every way of building settings must pass, run again by
    /// [`PeerLink::spawn`] so settings written as a literal cannot skip
    /// them: between one and [`MAX_EDGE_PEERS`] peers; under plaintext,
    /// every address loopback; under mutual TLS, every peer name a DNS name
    /// a certificate can carry.
    pub fn validate(&self) -> Result<()> {
        if self.peers.is_empty() || self.peers.len() > MAX_EDGE_PEERS {
            return Err(Error::invalid(format!(
                "configuration: {PEERS_VARIABLE} names {} peers; between one and \
                 {MAX_EDGE_PEERS}",
                self.peers.len()
            )));
        }
        if self.security.is_mutual_tls() {
            PeerNames::new(self.peers.keys().cloned()).map_err(|error| {
                Error::invalid(format!(
                    "configuration: {PEERS_VARIABLE}: under mutual TLS each peer is named by the \
                     DNS name its certificate carries: {}",
                    error.message()
                ))
            })?;
            return Ok(());
        }
        let addresses = std::iter::once((LISTEN_VARIABLE, &self.listen))
            .chain(self.peers.values().map(|at| (PEERS_VARIABLE, at)));
        for (variable, at) in addresses {
            if !at.ip().is_loopback() {
                return Err(Error::invalid(format!(
                    "configuration: {variable}: `{at}` is not loopback, and this peer link is \
                     plaintext. The peer mesh crosses hosts only under mutual TLS (ADR 0112): set \
                     the workload certificate and leave QIP_TLS_MODE unset or `mtls`. Plaintext \
                     frames carrying reservations and fills are admitted on loopback alone"
                )));
            }
        }
        Ok(())
    }
}

fn address(variable: &str, value: &str) -> Result<SocketAddr> {
    value.trim().parse().map_err(|_| {
        Error::invalid(format!(
            "configuration: {variable}: `{value}` is not an ip:port address"
        ))
    })
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
    handshakes_refused: AtomicU64,
}

/// A copy of [`PeerCounters`] at one instant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PeerWireStats {
    /// Frame copies written to a peer's socket.
    pub sent: u64,
    /// Writes or connects that failed; the peer then backs off.
    pub send_failures: u64,
    /// Frames the session offered while the outbound queue was full.
    pub queued_drops: u64,
    /// Whole frames read from peers and handed to the session.
    pub received: u64,
    /// Frames read while the inbound queue was full.
    pub inbound_drops: u64,
    /// Connections closed for declaring a frame over [`MAX_FRAME`].
    pub oversized: u64,
    /// Inbound TLS connections closed for a failed or stalled handshake, or
    /// a certificate naming no configured peer.
    pub handshakes_refused: u64,
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
            handshakes_refused: self.handshakes_refused.load(Ordering::Relaxed),
        }
    }
}

/// One whole frame read off the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboundFrame {
    /// The peer the connection authenticated as under mutual TLS; `None` on
    /// a plaintext link, where nobody was authenticated.
    pub from: Option<String>,
    /// When the wire finished reading it, on the link's clock. A clock echo
    /// is timed by this rather than by when the session got round to it, so
    /// the session's own queueing is not mistaken for path delay.
    pub arrived: Timestamp,
    pub frame: Vec<u8>,
}

struct Outgoing {
    /// `None` is every peer.
    to: Option<Vec<String>>,
    frame: Vec<u8>,
}

/// The session's end of the wire.
#[derive(Debug)]
pub struct PeerLink {
    local: SocketAddr,
    outbound: SyncSender<Outgoing>,
    inbound: Receiver<InboundFrame>,
    counters: Arc<PeerCounters>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl std::fmt::Debug for Outgoing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Outgoing")
            .field("to", &self.to)
            .field("bytes", &self.frame.len())
            .finish()
    }
}

/// The TLS halves the wire needs, built once from the workload identity.
struct Tls {
    server: Arc<ServerConfig>,
    client: Arc<ClientConfig>,
    names: PeerNames,
}

impl PeerLink {
    /// Bind `settings.listen` and start the wire's thread, stamping inbound
    /// frames with the system clock.
    pub fn spawn(settings: &PeerSettings) -> Result<Self> {
        Self::spawn_with_clock(settings, Arc::new(SystemClock))
    }

    /// The same, stamping inbound frames with `clock`.
    pub fn spawn_with_clock(settings: &PeerSettings, clock: Arc<dyn Clock>) -> Result<Self> {
        settings.validate()?;
        let tls = match &settings.security {
            LinkSecurity::Plaintext => None,
            LinkSecurity::MutualTls(identity) => Some(Tls {
                server: identity.server_config()?,
                client: identity.client_config()?,
                names: PeerNames::new(settings.peers.keys().cloned())?,
            }),
        };
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
        let (outbound, to_wire) = sync_channel::<Outgoing>(QUEUE);
        let (from_wire, inbound) = sync_channel::<InboundFrame>(QUEUE);
        let counters = Arc::new(PeerCounters::default());
        let stop = Arc::new(AtomicBool::new(false));
        let mut wire = Wire {
            listener,
            tls,
            clock,
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
        self.enqueue(Outgoing { to: None, frame })
    }

    /// Offer one frame to the named peers only (MESH-041): an update about
    /// an epoch goes to the nodes holding a leg of it, never to every node.
    pub fn offer_to(&self, peers: Vec<String>, frame: Vec<u8>) -> bool {
        self.enqueue(Outgoing {
            to: Some(peers),
            frame,
        })
    }

    fn enqueue(&self, outgoing: Outgoing) -> bool {
        match self.outbound.try_send(outgoing) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.counters.queued_drops.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Every whole frame read since the last call, oldest first. Never waits.
    pub fn take_inbound(&self) -> Vec<InboundFrame> {
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

/// An outbound connection: a socket, or a socket inside a client session
/// whose handshake completed before it was stored.
enum Stream {
    Plain(TcpStream),
    Tls(Box<ClientConnection>, TcpStream),
}

impl Stream {
    /// Write one whole frame. The socket is blocking with a write timeout,
    /// and only the encrypting side of the session is driven: nothing here
    /// reads, so a peer that stops reading costs a timeout and no more.
    fn write_frame(&mut self, frame: &[u8]) -> std::io::Result<()> {
        match self {
            Self::Plain(stream) => stream.write_all(frame).and_then(|()| stream.flush()),
            Self::Tls(session, stream) => {
                session.writer().write_all(frame)?;
                while session.wants_write() {
                    session.write_tls(stream)?;
                }
                stream.flush()
            }
        }
    }
}

struct Outbound {
    address: SocketAddr,
    stream: Option<Stream>,
    retry_at: Option<std::time::Instant>,
}

struct Inbound {
    stream: TcpStream,
    tls: Option<Box<ServerConnection>>,
    /// The peer the handshake authenticated; set once, before any frame is
    /// parsed on a TLS connection.
    peer: Option<String>,
    accepted: std::time::Instant,
    buffer: Vec<u8>,
    open: bool,
}

struct Wire {
    listener: TcpListener,
    tls: Option<Tls>,
    clock: Arc<dyn Clock>,
    peers: BTreeMap<String, Outbound>,
    inbound: Vec<Inbound>,
    to_wire: Receiver<Outgoing>,
    from_wire: SyncSender<InboundFrame>,
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
                // The session's end is gone: nothing more to carry.
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
                    let tls = match &self.tls {
                        None => None,
                        Some(tls) => match ServerConnection::new(Arc::clone(&tls.server)) {
                            Ok(session) => Some(Box::new(session)),
                            Err(_) => {
                                self.counters
                                    .handshakes_refused
                                    .fetch_add(1, Ordering::Relaxed);
                                continue;
                            }
                        },
                    };
                    self.inbound.push(Inbound {
                        stream,
                        tls,
                        peer: None,
                        accepted: std::time::Instant::now(),
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
            let progressed = match connection.tls.as_mut() {
                None => read_plain(connection, &mut chunk),
                Some(_) => read_tls(connection, &mut chunk),
            };
            any |= progressed;
            if let (Some(session), Some(tls)) = (connection.tls.as_ref(), self.tls.as_ref()) {
                if session.is_handshaking() && !connection.open {
                    // A connection that failed or closed before proving who
                    // it is: plaintext at the TLS listener, a certificate the
                    // roots do not chain, or a peer that gave up.
                    self.counters
                        .handshakes_refused
                        .fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                if session.is_handshaking() {
                    if connection.accepted.elapsed() > HANDSHAKE_TIMEOUT {
                        self.counters
                            .handshakes_refused
                            .fetch_add(1, Ordering::Relaxed);
                        connection.open = false;
                    }
                    continue;
                }
                if connection.peer.is_none() {
                    // The certificate chained to the roots in the handshake;
                    // here it must also name one of this node's peers.
                    let admitted = session
                        .peer_certificates()
                        .and_then(|chain| chain.first())
                        .and_then(|leaf| tls.names.admit(leaf).ok().flatten());
                    match admitted {
                        Some(name) => connection.peer = Some(name),
                        None => {
                            self.counters
                                .handshakes_refused
                                .fetch_add(1, Ordering::Relaxed);
                            connection.open = false;
                            continue;
                        }
                    }
                }
            }
            while connection.open && connection.buffer.len() >= 4 {
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
                let inbound = InboundFrame {
                    from: connection.peer.clone(),
                    arrived: self.clock.now(),
                    frame,
                };
                match self.from_wire.try_send(inbound) {
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

    /// `None` once the session has dropped its end.
    fn send(&mut self) -> Option<bool> {
        let mut any = false;
        loop {
            let outgoing = match self.to_wire.try_recv() {
                Ok(outgoing) => outgoing,
                Err(TryRecvError::Empty) => return Some(any),
                Err(TryRecvError::Disconnected) => return None,
            };
            any = true;
            let now = std::time::Instant::now();
            let client = self.tls.as_ref().map(|tls| Arc::clone(&tls.client));
            for (name, peer) in &mut self.peers {
                if let Some(to) = &outgoing.to
                    && !to.contains(name)
                {
                    continue;
                }
                if peer.stream.is_none() {
                    if peer.retry_at.is_some_and(|at| now < at) {
                        self.counters.send_failures.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    match connect(name, peer.address, client.as_ref()) {
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
                    .map(|stream| stream.write_frame(&outgoing.frame));
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

/// Read what a plaintext connection has, without waiting.
fn read_plain(connection: &mut Inbound, chunk: &mut [u8]) -> bool {
    let mut any = false;
    loop {
        match connection.stream.read(chunk) {
            Ok(0) => {
                connection.open = false;
                break;
            }
            Ok(read) => {
                any = true;
                connection.buffer.extend_from_slice(&chunk[..read]);
                // Bounded: a connection holds at most one frame and its
                // prefix beyond what has been parsed.
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
    any
}

/// Drive a server session on a non-blocking socket: read records, process
/// them, take the plaintext, and write whatever the handshake owes. Never
/// waits.
fn read_tls(connection: &mut Inbound, chunk: &mut [u8]) -> bool {
    let Some(session) = connection.tls.as_mut() else {
        return false;
    };
    let mut any = false;
    loop {
        match session.read_tls(&mut connection.stream) {
            Ok(0) => {
                connection.open = false;
                break;
            }
            Ok(_) => {
                any = true;
                if session.process_new_packets().is_err() {
                    // Owe the peer the alert, best effort, and close.
                    let _ = session.write_tls(&mut connection.stream);
                    connection.open = false;
                    break;
                }
                loop {
                    match session.reader().read(chunk) {
                        Ok(0) => {
                            connection.open = false;
                            break;
                        }
                        Ok(read) => {
                            connection.buffer.extend_from_slice(&chunk[..read]);
                            if connection.buffer.len() > 2 * (MAX_FRAME + 4) {
                                connection.open = false;
                                break;
                            }
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                        Err(_) => {
                            connection.open = false;
                            break;
                        }
                    }
                }
                if !connection.open {
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
    while connection.open && session.wants_write() {
        match session.write_tls(&mut connection.stream) {
            Ok(_) => any = true,
            Err(error) if error.kind() == ErrorKind::WouldBlock => break,
            Err(_) => connection.open = false,
        }
    }
    any
}

/// Connect to `name` at `address`, completing a TLS handshake that verifies
/// the peer's certificate for `name` when `client` is given. Bounded by the
/// connect, write and handshake timeouts.
fn connect(
    name: &str,
    address: SocketAddr,
    client: Option<&Arc<ClientConfig>>,
) -> std::io::Result<Stream> {
    let stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    stream.set_nodelay(true)?;
    let Some(client) = client else {
        return Ok(Stream::Plain(stream));
    };
    let invalid = |error: String| std::io::Error::new(ErrorKind::InvalidInput, error);
    let server = ServerName::try_from(name.to_string()).map_err(|e| invalid(e.to_string()))?;
    let mut session =
        ClientConnection::new(Arc::clone(client), server).map_err(|e| invalid(e.to_string()))?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let mut stream = stream;
    let began = std::time::Instant::now();
    while session.is_handshaking() {
        if began.elapsed() > HANDSHAKE_TIMEOUT {
            return Err(std::io::Error::new(
                ErrorKind::TimedOut,
                "the peer's TLS handshake did not complete in time",
            ));
        }
        session.complete_io(&mut stream)?;
    }
    Ok(Stream::Tls(Box::new(session), stream))
}

/// The limits this node's endpoint enforces, with the default clock bound.
pub fn node_limits() -> PeerLimits {
    limits_with(DEFAULT_CLOCK_BOUND)
}

/// The limits this node's endpoint enforces under `clock_bound`.
///
/// The endpoint joins an epoch only with every counterparty's clock
/// measured (probe and echo, [`PeerSession::probe`]) inside this bound, or
/// refuses it by name (`clock_unknown`, `clock_out_of_bound`) rather than
/// assuming an offset of zero.
pub fn limits_with(clock_bound: Duration) -> PeerLimits {
    PeerLimits {
        max_frame: MAX_FRAME,
        ttl_ceiling: Duration::from_secs(30),
        max_clock_offset: clock_bound,
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
        Self::with_clock(settings, Arc::new(SystemClock))
    }

    /// A session whose wire stamps arrivals with `clock` — the clock its
    /// caller passes as `now`, so a clock echo's four instants are on two
    /// clocks and not three.
    pub fn with_clock(settings: &PeerSettings, clock: Arc<dyn Clock>) -> Result<Self> {
        Ok(Self {
            endpoint: PeerEndpoint::new(&settings.node, limits_with(settings.clock_bound)),
            link: PeerLink::spawn_with_clock(settings, clock)?,
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

    /// Offer a protocol frame to every peer.
    pub fn send(&self, frame: Vec<u8>) -> bool {
        self.link.offer(frame)
    }

    /// Offer a protocol frame to the named peers only.
    pub fn send_to(&self, peers: Vec<String>, frame: Vec<u8>) -> bool {
        self.link.offer_to(peers, frame)
    }

    /// Probe every peer's clock (MESH-007).
    pub fn probe(&mut self, now: Timestamp) -> Result<bool> {
        let frame = self.endpoint.clock_probe_frame(now)?;
        Ok(self.link.offer(frame))
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

    /// Answer the clock probes absorbed since the last call, each echo
    /// addressed to its prober alone.
    pub fn answer_probes(&mut self, now: Timestamp) -> usize {
        let echoes = self.endpoint.take_echoes(now);
        let count = echoes.len();
        for (peer, frame) in echoes {
            self.link.offer_to(vec![peer], frame);
        }
        count
    }

    fn absorb(&mut self, now: Timestamp) -> PeerTurn {
        let mut turn = PeerTurn::default();
        for inbound in self.link.take_inbound() {
            turn.received += 1;
            // Timed at arrival, and never after the turn that applies it.
            let at = if inbound.arrived <= now {
                inbound.arrived
            } else {
                now
            };
            let admitted = match &inbound.from {
                Some(from) => self.endpoint.receive_from(&inbound.frame, from, at),
                None => self.endpoint.receive(&inbound.frame, at),
            };
            if admitted.is_err() {
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

    fn plaintext() -> Result<LinkSecurity> {
        Ok(LinkSecurity::plaintext_for_tests())
    }

    /// Mutation: drop the loopback check from `validate` and the first
    /// assertion fails.
    #[test]
    fn a_plaintext_peer_off_loopback_is_refused_and_a_loopback_one_is_admitted() {
        let error = PeerSettings::read(
            "a",
            Some("127.0.0.1:0"),
            Some("b=10.0.0.2:7000"),
            None,
            &plaintext,
        )
        .expect_err("a non-loopback plaintext peer must be refused");
        assert!(error.message().contains("ADR 0112"), "{}", error.message());
        assert!(
            PeerSettings::read(
                "a",
                Some("0.0.0.0:7000"),
                Some("b=127.0.0.1:7001"),
                None,
                &plaintext
            )
            .is_err(),
            "listening on every interface is not loopback"
        );
        assert!(
            PeerSettings::read("a", None, None, None, &plaintext)
                .expect("unset")
                .is_none()
        );
        assert!(PeerSettings::read("a", Some("127.0.0.1:0"), None, None, &plaintext).is_err());
        assert!(
            PeerSettings::read(
                "a",
                Some("127.0.0.1:0"),
                Some("a=127.0.0.1:1"),
                None,
                &plaintext
            )
            .is_err()
        );
        let settings = PeerSettings::read(
            "a",
            Some("127.0.0.1:0"),
            Some("b=127.0.0.1:7001"),
            None,
            &plaintext,
        )
        .expect("well formed")
        .expect("configured");
        assert_eq!(settings.peers.len(), 1);
        assert_eq!(settings.clock_bound, DEFAULT_CLOCK_BOUND);
    }

    /// The security decision is asked for only when the mesh is configured:
    /// a node with no peers needs no certificate. And a bound outside its
    /// range is refused rather than clamped.
    ///
    /// Mutation: call `security()` before the unset check and the first
    /// assertion fails; clamp the bound and the second does.
    #[test]
    fn an_unconfigured_mesh_asks_for_no_certificate_and_a_clock_bound_out_of_range_is_refused() {
        let refuse = || -> Result<LinkSecurity> { Err(Error::invalid("no certificate")) };
        assert!(
            PeerSettings::read("a", None, None, None, &refuse)
                .expect("an unconfigured mesh needs no certificate")
                .is_none()
        );
        for bad in ["0", "1001", "-3", "five"] {
            assert!(
                PeerSettings::read(
                    "a",
                    Some("127.0.0.1:0"),
                    Some("b=127.0.0.1:7001"),
                    Some(bad),
                    &plaintext
                )
                .is_err(),
                "clock bound {bad} was admitted"
            );
        }
        let wide = PeerSettings::read(
            "a",
            Some("127.0.0.1:0"),
            Some("b=127.0.0.1:7001"),
            Some("80"),
            &plaintext,
        )
        .expect("in range")
        .expect("configured");
        assert_eq!(wide.clock_bound, Duration::from_millis(80));
    }
}
