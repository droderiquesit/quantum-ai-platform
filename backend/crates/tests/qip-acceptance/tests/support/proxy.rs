//! A seeded, four-fault TCP proxy for the slice suites' partition tests.
//!
//! A suite that wants to know a process behaves correctly when its peer stops
//! answering cannot get that from killing the peer outright — that is a
//! different, already-tested failure. It needs a real socket in between that
//! it can turn hostile on command, replaying the same sequence of faults on
//! every run so a failure is something to reproduce rather than something
//! that happened once.
//!
//! Four faults, because "the network is unreliable" is not one condition:
//!
//! * [`FaultKind::Stall`] — accept and hold, forever. Models a peer that
//!   completed the handshake and then wedged before touching the socket.
//! * [`FaultKind::Blackhole`] — accept, read and discard everything, answer
//!   nothing. Models a peer that consumes input and produces no output.
//! * [`FaultKind::DropAcks`] — forward the request to a real upstream, and
//!   throw away its reply. Models a request that is genuinely acted on
//!   somewhere whose acknowledgement never makes it back.
//! * [`FaultKind::Cut`] — sever the connection the instant it is accepted.
//!   Models the peer process vanishing mid-handshake.
//!
//! Only `Stall` carries a dedicated behavioural test in this packet
//! (`a_stalled_upstream_accepts_the_connection_and_never_answers`); the other
//! three are implemented to the same standard but proven by whichever future
//! suite is the first to drive a real process through them — stated here
//! rather than left for a reader to discover the hard way.
//!
//! This workspace forbids raw sockets and hand-rolled protocol stacks
//! (ADR 0009), so "drop acks" is not a literal TCP-ACK manipulation — that
//! would need a raw socket this crate is not permitted to open. It is the
//! nearest honest approximation at the application layer: the request gets
//! through, the acknowledgement does not.

use qip_core::Rng as _;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// One fault a [`Proxy`] can apply to an accepted connection.
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

/// A deterministic sequence of faults, one per connection a [`Proxy`] would
/// accept.
///
/// Built from a seed so that replaying a run means passing the same number
/// back in, never recording the schedule itself: "same seed, same schedule"
/// is the property `one_seed_yields_one_fault_schedule` exists to prove, and
/// the reason nothing in this module reads the wall clock.
pub(crate) struct FaultSchedule {
    kinds: Vec<FaultKind>,
}

impl FaultSchedule {
    pub(crate) fn seeded(seed: u64, len: usize) -> Self {
        let mut rng = qip_core::Xoshiro256::seeded(seed);
        let kinds = (0..len)
            .map(|_| FaultKind::from_index(rng.below(4)))
            .collect();
        Self { kinds }
    }

    pub(crate) fn kinds(&self) -> &[FaultKind] {
        &self.kinds
    }
}

/// A TCP proxy on an ephemeral loopback port, applying one fault to every
/// connection it accepts.
pub(crate) struct Proxy {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    held: Arc<Mutex<Vec<TcpStream>>>,
    handle: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proxy")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

impl Proxy {
    /// A proxy applying `fault` to every connection it accepts.
    ///
    /// `upstream` is required for [`FaultKind::DropAcks`], which has nowhere
    /// to forward a request without one; refused at construction rather than
    /// silently downgraded to a different fault, per this workspace's rule
    /// that an invalid input is refused, not clamped.
    pub(crate) fn single_fault(fault: FaultKind, upstream: Option<SocketAddr>) -> Self {
        assert!(
            fault != FaultKind::DropAcks || upstream.is_some(),
            "FaultKind::DropAcks forwards a request to a real upstream and has none to discard a \
             reply from; pass Some(address) or choose a different fault"
        );

        let listener =
            TcpListener::bind("127.0.0.1:0").expect("bind a loopback port for the fault proxy");
        listener
            .set_nonblocking(true)
            .expect("the listener can poll");
        let address = listener
            .local_addr()
            .expect("a bound listener has a local address");

        let stop = Arc::new(AtomicBool::new(false));
        let held: Arc<Mutex<Vec<TcpStream>>> = Arc::new(Mutex::new(Vec::new()));
        let thread_stop = Arc::clone(&stop);
        let thread_held = Arc::clone(&held);

        let handle = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        apply_fault(fault, stream, upstream, &thread_held);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            address,
            stop,
            held,
            handle: Some(handle),
        }
    }

    pub(crate) fn address(&self) -> SocketAddr {
        self.address
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        // Every `Stall`ed connection is closed here, on the proxy's own
        // teardown, and at no point earlier — holding it open until exactly
        // this moment is the fault. A per-connection worker thread for
        // `Blackhole` or `DropAcks` is not tracked here and instead exits on
        // its own once the peer closes its side; an honest limit rather than
        // an oversight, stated because a suite driving either of those faults
        // through a real process is the thing that will next need to know it.
        self.held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

fn apply_fault(
    fault: FaultKind,
    stream: TcpStream,
    upstream: Option<SocketAddr>,
    held: &Arc<Mutex<Vec<TcpStream>>>,
) {
    match fault {
        FaultKind::Stall => {
            // The failure this prevents: returning early here drops `stream`,
            // which closes the socket — indistinguishable from `Cut` to the
            // peer, and not what a wedged-but-alive process looks like.
            // Parking it in `held` keeps the file descriptor open until the
            // proxy itself is torn down.
            held.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(stream);
        }
        FaultKind::Blackhole => {
            std::thread::spawn(move || drain_forever(stream));
        }
        FaultKind::DropAcks => {
            let Some(upstream_address) = upstream else {
                // Refused at construction above; reachable only if that
                // check is ever removed, and dropping the connection is the
                // fail-closed response if it is.
                return;
            };
            std::thread::spawn(move || forward_discarding_reply(stream, upstream_address));
        }
        FaultKind::Cut => {
            drop(stream);
        }
    }
}

/// Read and discard until the peer closes or errors. Never writes back.
fn drain_forever(mut stream: TcpStream) {
    let mut sink = [0u8; 4096];
    loop {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

/// Forward one request to `upstream_address` and read its reply only to throw
/// it away, never relaying it to `client`.
fn forward_discarding_reply(mut client: TcpStream, upstream_address: SocketAddr) {
    let Ok(mut upstream) = TcpStream::connect(upstream_address) else {
        return;
    };
    let _ = client.set_read_timeout(Some(std::time::Duration::from_millis(500)));
    let mut buffer = [0u8; 4096];
    // One read is enough for the small, single-shot requests these suites
    // send; a general-purpose reverse proxy would loop until the peer
    // half-closes, but this exists to prove one fault, not to replace one.
    if let Ok(n) = client.read(&mut buffer) {
        let _ = upstream.write_all(&buffer[..n]);
    }
    let mut discard = [0u8; 4096];
    loop {
        match upstream.read(&mut discard) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}
