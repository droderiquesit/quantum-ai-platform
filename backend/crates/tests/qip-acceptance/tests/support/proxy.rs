//! A fault-injecting TCP relay for the slice suites' partition tests.
//!
//! A suite that wants to know how a process behaves when its peer misbehaves
//! cannot get that from killing the peer — that is a different, separately
//! tested failure, a refused connection. It needs a real socket in between
//! that relays faithfully until the suite has seen the healthy state it
//! depends on (a grant applied, a series moved), then turns hostile on
//! command, and back.
//!
//! [`Proxy::start`] relays both directions, byte for byte, until told
//! otherwise. [`Proxy::inject`] switches every connection — open, and yet to
//! be accepted — to one fault at once; [`Proxy::heal`] switches back.
//! [`Proxy::inject_next`] draws the next fault from a [`FaultSchedule`]
//! seeded once, so a run that failed replays by passing the same seed back
//! rather than by recording what happened.
//!
//! Four faults, because "the network is unreliable" is not one condition:
//!
//! * [`FaultKind::Stall`] — accept and hold. Nothing is read or written in
//!   either direction, so the peer's writes back up once the kernel's buffers
//!   fill. Models a process that took the connection and wedged. Healing
//!   resumes the same connections with nothing lost: a stall is a pause.
//! * [`FaultKind::Blackhole`] — accept, read and discard everything, answer
//!   nothing and forward nothing. Models a peer that consumes input and
//!   produces no output.
//! * [`FaultKind::DropAcks`] — the request reaches the upstream and is acted
//!   on; the reply is read and discarded. The work was done and its
//!   acknowledgement never arrives, which is what makes a client retry
//!   something that already happened.
//! * [`FaultKind::Cut`] — every open connection severed, and every new one
//!   closed as soon as it is accepted, without reaching the upstream.
//!
//! **A stream with a hole in it is never resumed.** Once `Blackhole` or
//! `DropAcks` has discarded bytes on a connection, the direction that lost
//! them never relays again, and healing closes that connection rather than
//! relaying on from the middle of a message. TCP never delivers a stream with
//! a gap in it, so resuming one would test the peer's parser against an input
//! no network produces instead of testing its retry path. A retry after
//! healing goes out on a fresh connection and gets through.
//!
//! These are the application layer's versions of the four faults. This
//! workspace forbids raw sockets and hand-rolled protocol stacks (ADR 0009),
//! so "drop acks" is not TCP-ACK manipulation — that needs a raw socket this
//! crate may not open. The request gets through; the acknowledgement does not.

use qip_core::Rng as _;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

/// How long a relay thread blocks in one read, or one wait on a stalled
/// connection, before it looks at the fault state again. Bounds how late an
/// injection reaches a connection; a read that has data returns at once, so
/// it adds no latency to a healthy relay.
const TICK: Duration = Duration::from_millis(10);

/// How long the proxy waits to reach its upstream before closing the client's
/// connection — which is how an unreachable upstream looks through it.
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

const BUFFER_BYTES: usize = 16 * 1024;

/// One fault a [`Proxy`] can apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FaultKind {
    Stall,
    Blackhole,
    DropAcks,
    Cut,
}

impl FaultKind {
    fn from_index(index: u64) -> Self {
        match index % 4 {
            0 => Self::Stall,
            1 => Self::Blackhole,
            2 => Self::DropAcks,
            _ => Self::Cut,
        }
    }
}

/// A deterministic sequence of faults for a suite to step a [`Proxy`]
/// through.
///
/// Built from a seed so that replaying a run means passing the same number
/// back in, never recording the schedule itself: "same seed, same schedule"
/// is the property `one_seed_yields_one_fault_schedule` proves, and the
/// reason nothing in this module reads the wall clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FaultSchedule {
    kinds: Vec<FaultKind>,
    next: usize,
}

impl FaultSchedule {
    pub(crate) fn seeded(seed: u64, len: usize) -> Self {
        let mut rng = qip_core::Xoshiro256::seeded(seed);
        let kinds = (0..len)
            .map(|_| FaultKind::from_index(rng.below(4)))
            .collect();
        Self { kinds, next: 0 }
    }

    /// The whole schedule, including faults already drawn.
    pub(crate) fn kinds(&self) -> &[FaultKind] {
        &self.kinds
    }

    /// The next fault, or `None` once every one has been drawn.
    pub(crate) fn next_fault(&mut self) -> Option<FaultKind> {
        let fault = self.kinds.get(self.next).copied();
        if fault.is_some() {
            self.next += 1;
        }
        fault
    }
}

/// What a [`Proxy`] has done so far, for a suite to poll on as the observable
/// proof that a fault took effect — that an acknowledgement really was
/// dropped before it heals, say — rather than sleeping and hoping.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ProxyStats {
    /// Connections accepted from clients, whatever then happened to them.
    pub(crate) accepted: u64,
    /// Bytes relayed from a client to the upstream.
    pub(crate) forwarded_to_upstream: u64,
    /// Bytes relayed from the upstream to a client.
    pub(crate) forwarded_to_client: u64,
    /// Bytes read from a client and thrown away.
    pub(crate) discarded_from_client: u64,
    /// Bytes read from the upstream and thrown away.
    pub(crate) discarded_from_upstream: u64,
    /// Connections a fault closed: a cut, or a healed stream with a hole.
    /// Counted before the connection closes, so a test that has seen the
    /// close can read it at once. The byte counts above are taken after the
    /// read or write they describe, so a byte the peer has received may not
    /// be counted yet: poll those with [`super::poll_until`].
    pub(crate) severed: u64,
}

/// A TCP relay on an ephemeral loopback port in front of one upstream.
#[derive(Debug)]
pub(crate) struct Proxy {
    address: SocketAddr,
    shared: Arc<Shared>,
    acceptor: Option<JoinHandle<()>>,
}

impl Proxy {
    /// A healthy relay to `upstream`, listening on a free loopback port.
    pub(crate) fn start(upstream: SocketAddr) -> Self {
        let listener =
            TcpListener::bind("127.0.0.1:0").expect("bind a loopback port for the fault proxy");
        let address = listener
            .local_addr()
            .expect("a bound listener has a local address");
        let shared = Arc::new(Shared::new(upstream));
        let for_acceptor = Arc::clone(&shared);
        let acceptor = std::thread::spawn(move || accept_loop(&listener, &for_acceptor));
        Self {
            address,
            shared,
            acceptor: Some(acceptor),
        }
    }

    /// Where clients connect instead of the upstream.
    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }

    /// Apply `fault` to every open connection and every one accepted from now
    /// on, until [`heal`](Self::heal) or another injection.
    pub(crate) fn inject(&self, fault: FaultKind) {
        self.shared.set_fault(Some(fault));
    }

    /// Draw the next fault from `schedule` and inject it, returning which.
    ///
    /// Panics when the schedule is exhausted: a suite that asked for more
    /// faults than it seeded has a bug, and inventing one would break the
    /// replay the seed exists for.
    pub(crate) fn inject_next(&self, schedule: &mut FaultSchedule) -> FaultKind {
        let fault = schedule.next_fault().unwrap_or_else(|| {
            panic!(
                "the fault schedule is exhausted after {} faults; seed a longer one",
                schedule.kinds().len()
            )
        });
        self.inject(fault);
        fault
    }

    /// Relay faithfully again. Stalled connections resume where they paused;
    /// a connection that lost bytes to a fault is closed instead.
    pub(crate) fn heal(&self) {
        self.shared.set_fault(None);
    }

    /// The fault now in force, if any.
    pub(crate) fn fault(&self) -> Option<FaultKind> {
        self.shared.fault()
    }

    pub(crate) fn stats(&self) -> ProxyStats {
        let counters = &self.shared.counters;
        ProxyStats {
            accepted: counters.accepted.load(Ordering::SeqCst),
            forwarded_to_upstream: counters.forwarded_to_upstream.load(Ordering::SeqCst),
            forwarded_to_client: counters.forwarded_to_client.load(Ordering::SeqCst),
            discarded_from_client: counters.discarded_from_client.load(Ordering::SeqCst),
            discarded_from_upstream: counters.discarded_from_upstream.load(Ordering::SeqCst),
            severed: counters.severed.load(Ordering::SeqCst),
        }
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.shared.stop();
        // `accept` blocks, so one connection of our own is what returns it to
        // find `stopping` set. If even that cannot connect the acceptor is
        // left detached rather than joined: a hung teardown would hang the
        // suite, and a thread blocked in `accept` holds no child process.
        let woke = TcpStream::connect_timeout(&self.address, Duration::from_secs(1)).is_ok();
        if let Some(acceptor) = self.acceptor.take()
            && woke
        {
            let _ = acceptor.join();
        }
        // Every connection closed and every relay thread joined, stalled ones
        // included: nothing this proxy started outlives it.
        let links = std::mem::take(&mut *lock(&self.shared.links));
        for (link, _) in &links {
            link.sever();
        }
        for (_, worker) in links {
            let _ = worker.join();
        }
    }
}

// --- the shared state every relay thread consults ----------------------------

#[derive(Debug)]
struct Control {
    fault: Option<FaultKind>,
    stopping: bool,
}

#[derive(Debug, Default)]
struct Counters {
    accepted: AtomicU64,
    forwarded_to_upstream: AtomicU64,
    forwarded_to_client: AtomicU64,
    discarded_from_client: AtomicU64,
    discarded_from_upstream: AtomicU64,
    severed: AtomicU64,
}

#[derive(Debug)]
struct Shared {
    upstream: SocketAddr,
    control: Mutex<Control>,
    changed: Condvar,
    counters: Counters,
    links: Mutex<Vec<(Arc<Link>, JoinHandle<()>)>>,
}

impl Shared {
    fn new(upstream: SocketAddr) -> Self {
        Self {
            upstream,
            control: Mutex::new(Control {
                fault: None,
                stopping: false,
            }),
            changed: Condvar::new(),
            counters: Counters::default(),
            links: Mutex::new(Vec::new()),
        }
    }

    fn fault(&self) -> Option<FaultKind> {
        lock(&self.control).fault
    }

    fn stopping(&self) -> bool {
        lock(&self.control).stopping
    }

    fn set_fault(&self, fault: Option<FaultKind>) {
        lock(&self.control).fault = fault;
        self.changed.notify_all();
    }

    fn stop(&self) {
        lock(&self.control).stopping = true;
        self.changed.notify_all();
    }

    /// Wait, at most one tick, for a stall to end. Woken at once by
    /// [`set_fault`](Self::set_fault) or [`stop`](Self::stop); the tick only
    /// bounds how late a connection severed underneath it is noticed.
    fn hold(&self) {
        let control = lock(&self.control);
        let _ = self
            .changed
            .wait_timeout_while(control, TICK, |control| {
                control.fault == Some(FaultKind::Stall) && !control.stopping
            })
            .unwrap_or_else(PoisonError::into_inner);
    }

    fn count(&self, counter: &AtomicU64, bytes: usize) {
        counter.fetch_add(u64::try_from(bytes).unwrap_or(u64::MAX), Ordering::SeqCst);
    }

    fn forwarded(&self, direction: Direction, bytes: usize) {
        let counter = match direction {
            Direction::ToUpstream => &self.counters.forwarded_to_upstream,
            Direction::ToClient => &self.counters.forwarded_to_client,
        };
        self.count(counter, bytes);
    }

    fn discarded(&self, direction: Direction, bytes: usize) {
        let counter = match direction {
            Direction::ToUpstream => &self.counters.discarded_from_client,
            Direction::ToClient => &self.counters.discarded_from_upstream,
        };
        self.count(counter, bytes);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// --- one accepted connection -------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    ToUpstream,
    ToClient,
}

/// What a direction does with its bytes under a given fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Forward,
    Hold,
    Discard,
    Sever,
}

fn action(fault: Option<FaultKind>, direction: Direction) -> Action {
    match (fault, direction) {
        (None, _) | (Some(FaultKind::DropAcks), Direction::ToUpstream) => Action::Forward,
        (Some(FaultKind::Stall), _) => Action::Hold,
        (Some(FaultKind::Blackhole), _) | (Some(FaultKind::DropAcks), Direction::ToClient) => {
            Action::Discard
        }
        (Some(FaultKind::Cut), _) => Action::Sever,
    }
}

#[derive(Debug)]
struct Link {
    client: TcpStream,
    /// A second handle on the upstream socket, held only so `sever` can shut
    /// it down from another thread.
    upstream: Mutex<Option<TcpStream>>,
    severed: AtomicBool,
    hole_to_upstream: AtomicBool,
    hole_to_client: AtomicBool,
}

impl Link {
    fn new(client: TcpStream) -> Self {
        let _ = client.set_read_timeout(Some(TICK));
        Self {
            client,
            upstream: Mutex::new(None),
            severed: AtomicBool::new(false),
            hole_to_upstream: AtomicBool::new(false),
            hole_to_client: AtomicBool::new(false),
        }
    }

    fn is_severed(&self) -> bool {
        self.severed.load(Ordering::SeqCst)
    }

    /// Close both sockets, once. A shutdown from here also wakes a relay
    /// thread blocked reading or writing either one, which is what lets
    /// teardown join them.
    fn sever(&self) {
        if !self.severed.swap(true, Ordering::SeqCst) {
            self.shut_down();
        }
    }

    /// Sever because a fault said to, counted once per connection — and
    /// counted *before* the sockets close. The other order let a client see
    /// its connection closed while `severed` still read the old count, and a
    /// suite that asserted the count on seeing the close failed one run in six
    /// under load.
    fn sever_for_fault(&self, shared: &Shared) {
        if !self.severed.swap(true, Ordering::SeqCst) {
            shared.counters.severed.fetch_add(1, Ordering::SeqCst);
            self.shut_down();
        }
    }

    fn shut_down(&self) {
        let _ = self.client.shutdown(Shutdown::Both);
        if let Some(upstream) = lock(&self.upstream).as_ref() {
            let _ = upstream.shutdown(Shutdown::Both);
        }
    }

    fn hole(&self, direction: Direction) -> &AtomicBool {
        match direction {
            Direction::ToUpstream => &self.hole_to_upstream,
            Direction::ToClient => &self.hole_to_client,
        }
    }

    fn has_hole(&self, direction: Direction) -> bool {
        self.hole(direction).load(Ordering::SeqCst)
    }

    fn mark_hole(&self, direction: Direction) {
        self.hole(direction).store(true, Ordering::SeqCst);
    }

    /// Record the upstream socket so [`sever`](Self::sever) can reach it,
    /// refusing it if the connection was severed while it was being dialled.
    fn attach(&self, upstream: &TcpStream) -> bool {
        let Ok(handle) = upstream.try_clone() else {
            return false;
        };
        *lock(&self.upstream) = Some(handle);
        if self.is_severed() {
            let _ = upstream.shutdown(Shutdown::Both);
            return false;
        }
        true
    }
}

fn accept_loop(listener: &TcpListener, shared: &Arc<Shared>) {
    for incoming in listener.incoming() {
        if shared.stopping() {
            return;
        }
        // A failed accept is a peer that reset before it was taken: that
        // peer's problem, not a reason to stop serving the rest.
        let Ok(stream) = incoming else {
            continue;
        };
        shared.counters.accepted.fetch_add(1, Ordering::SeqCst);
        if shared.fault() == Some(FaultKind::Cut) {
            shared.counters.severed.fetch_add(1, Ordering::SeqCst);
            let _ = stream.shutdown(Shutdown::Both);
            continue;
        }
        let link = Arc::new(Link::new(stream));
        let worker = {
            let shared = Arc::clone(shared);
            let link = Arc::clone(&link);
            std::thread::spawn(move || relay(&shared, &link))
        };
        let mut links = lock(&shared.links);
        // Finished connections are dropped as new ones arrive, so a suite
        // that reconnects in a loop does not grow this without bound.
        links.retain(|(_, worker)| !worker.is_finished());
        links.push((link, worker));
    }
}

/// Everything that happens to one accepted connection.
fn relay(shared: &Shared, link: &Link) {
    let Some(upstream) = dial_when_needed(shared, link) else {
        return;
    };
    let _ = upstream.set_read_timeout(Some(TICK));
    std::thread::scope(|scope| {
        scope.spawn(|| pump(shared, link, &upstream, &link.client, Direction::ToClient));
        pump(shared, link, &link.client, &upstream, Direction::ToUpstream);
    });
    link.sever();
}

/// Hold, discard or sever as the fault says until something would be
/// forwarded to the upstream, and only then dial it — so a connection
/// accepted while stalled, blackholed or cut never reaches the upstream at
/// all, which is the difference between those faults and a slow upstream.
fn dial_when_needed(shared: &Shared, link: &Link) -> Option<TcpStream> {
    let mut buffer = [0u8; BUFFER_BYTES];
    loop {
        if link.is_severed() || shared.stopping() {
            link.sever();
            return None;
        }
        match action(shared.fault(), Direction::ToUpstream) {
            Action::Sever => {
                link.sever_for_fault(shared);
                return None;
            }
            Action::Hold => shared.hold(),
            Action::Discard => match (&link.client).read(&mut buffer) {
                Ok(0) => {
                    link.sever();
                    return None;
                }
                Ok(n) => {
                    shared.discarded(Direction::ToUpstream, n);
                    link.mark_hole(Direction::ToUpstream);
                }
                Err(error) if is_tick(&error) => {}
                Err(_) => {
                    link.sever();
                    return None;
                }
            },
            Action::Forward => {
                if link.has_hole(Direction::ToUpstream) {
                    link.sever_for_fault(shared);
                    return None;
                }
                let Ok(upstream) =
                    TcpStream::connect_timeout(&shared.upstream, UPSTREAM_CONNECT_TIMEOUT)
                else {
                    link.sever();
                    return None;
                };
                return link.attach(&upstream).then_some(upstream);
            }
        }
    }
}

/// Move bytes one way until the connection ends, obeying whatever fault is in
/// force at each step.
///
/// Bytes read are held in `pending` and judged on the next turn, under the
/// fault in force *then*: a reply that arrives after `inject(DropAcks)` is
/// dropped even if the read that took it began while the relay was healthy.
fn pump(
    shared: &Shared,
    link: &Link,
    mut from: &TcpStream,
    mut to: &TcpStream,
    direction: Direction,
) {
    let mut pending: Vec<u8> = Vec::new();
    let mut buffer = [0u8; BUFFER_BYTES];
    loop {
        if link.is_severed() || shared.stopping() {
            return;
        }
        match action(shared.fault(), direction) {
            Action::Sever => {
                link.sever_for_fault(shared);
                return;
            }
            Action::Hold => shared.hold(),
            Action::Discard => {
                if !pending.is_empty() {
                    shared.discarded(direction, pending.len());
                    pending.clear();
                    link.mark_hole(direction);
                }
                match from.read(&mut buffer) {
                    Ok(0) => return,
                    Ok(n) => {
                        shared.discarded(direction, n);
                        link.mark_hole(direction);
                    }
                    Err(error) if is_tick(&error) => {}
                    Err(_) => {
                        link.sever();
                        return;
                    }
                }
            }
            Action::Forward => {
                if link.has_hole(direction) {
                    link.sever_for_fault(shared);
                    return;
                }
                if !pending.is_empty() {
                    if to.write_all(&pending).is_err() {
                        link.sever();
                        return;
                    }
                    shared.forwarded(direction, pending.len());
                    pending.clear();
                    continue;
                }
                match from.read(&mut buffer) {
                    Ok(0) => {
                        // A half-close passes through: the other side learns
                        // this one has finished sending, and may still answer.
                        let _ = to.shutdown(Shutdown::Write);
                        return;
                    }
                    Ok(n) => pending.extend_from_slice(&buffer[..n]),
                    Err(error) if is_tick(&error) => {}
                    Err(_) => {
                        link.sever();
                        return;
                    }
                }
            }
        }
    }
}

fn is_tick(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    )
}
