//! ADR 0100 §1: the `qip event-fabric …` subcommand family.
//!
//! `main.rs` routes the whole family here with one match arm and is not
//! edited again when the family grows: a later subcommand (SLICE-37's
//! `verify`, `inspect`, `lag`, `isolate`, `release`) is one more entry in
//! [`SUBCOMMANDS`] and one more arm in [`dispatch`], both in this file. The
//! alternative — one arm per subcommand in `main.rs` — is how a subcommand
//! that exists here ends up unreachable from the binary, answered by main's
//! generic "unknown command" while its own tests pass against the library.
//!
//! An unknown subcommand is refused here, naming the list, so an operator who
//! mistypes `qip event-fabric grnat` is told what the family holds rather
//! than that `event-fabric` is not a command.

pub mod grant;
pub mod inspect;

use qip_core::error::{Error, Result};
use qip_core::{Clock, SystemClock};
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::transport::FabricTransport;
use qip_transport::retry::{Sleeper, ThreadSleeper};
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

/// Every subcommand this family answers, in the order the refusal names them.
///
/// The one list [`dispatch`] matches and the unknown-subcommand refusal
/// prints, so the two cannot disagree about what exists.
pub const SUBCOMMANDS: [&str; 1] = [grant::SUBCOMMAND];

/// How a variable is looked up: the process environment in production, a
/// map in a test.
pub type Lookup = Box<dyn Fn(&str) -> Option<String>>;

/// How a transport to a checked peer is opened, carrying the identity every
/// call presents.
pub type Connector = Box<dyn Fn(SocketAddr, BearerToken) -> Box<dyn FabricTransport + Send>>;

/// Everything a family subcommand reaches outside its arguments.
///
/// Gathered so a test can supply each one and a subcommand never reads the
/// process environment, the wall clock or the network directly: a command
/// that did could only be tested by running a process against a broker, and
/// the refusals that matter most here — a non-loopback peer, a key in the
/// wrong place — would be the ones nobody exercised.
pub struct Environment {
    lookup: Lookup,
    clock: Arc<dyn Clock>,
    sleeper: Arc<dyn Sleeper>,
    connect: Connector,
}

impl Environment {
    /// The process's own environment, wall clock, blocking sleeper and HTTP
    /// transport.
    pub fn process() -> Self {
        Self {
            lookup: Box::new(|name| std::env::var(name).ok()),
            clock: Arc::new(SystemClock),
            sleeper: Arc::new(ThreadSleeper),
            connect: Box::new(grant::http_transport),
        }
    }

    pub fn new(
        lookup: Lookup,
        clock: Arc<dyn Clock>,
        sleeper: Arc<dyn Sleeper>,
        connect: Connector,
    ) -> Self {
        Self {
            lookup,
            clock,
            sleeper,
            connect,
        }
    }

    pub fn variable(&self, name: &str) -> Option<String> {
        (self.lookup)(name)
    }

    pub fn clock(&self) -> Arc<dyn Clock> {
        Arc::clone(&self.clock)
    }

    pub fn sleeper(&self) -> Arc<dyn Sleeper> {
        Arc::clone(&self.sleeper)
    }

    pub fn connect(
        &self,
        peer: SocketAddr,
        identity: BearerToken,
    ) -> Box<dyn FabricTransport + Send> {
        (self.connect)(peer, identity)
    }
}

impl fmt::Debug for Environment {
    /// Names nothing it could look up: a `{:?}` of this in an error path must
    /// not become the way a credential variable's value reaches a log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Environment")
            .field("clock", &self.clock)
            .field("sleeper", &self.sleeper)
            .finish_non_exhaustive()
    }
}

/// What a family subcommand decided: the lines to print and the exit code.
///
/// Returned rather than printed so a test asserts on what the operator
/// reads; [`run`] is the only place that prints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub lines: Vec<String>,
    pub code: u8,
}

/// `qip event-fabric <subcommand> …`, against the process's own environment,
/// clock and network. Prints what the subcommand decided and returns its
/// exit code for `main` to exit with.
pub fn run(arguments: &[String]) -> Result<u8> {
    let outcome = dispatch(arguments, &Environment::process())?;
    for line in &outcome.lines {
        println!("{line}");
    }
    Ok(outcome.code)
}

/// Route `arguments` — everything after `event-fabric` — to its subcommand.
pub fn dispatch(arguments: &[String], environment: &Environment) -> Result<Outcome> {
    let Some(subcommand) = arguments.first() else {
        return Err(Error::invalid(format!(
            "`qip event-fabric` needs a subcommand; the family is: {}",
            SUBCOMMANDS.join(", ")
        )));
    };
    match subcommand.as_str() {
        grant::SUBCOMMAND => grant::run(&arguments[1..], environment),
        other => Err(Error::invalid(format!(
            "unknown event-fabric subcommand {other:?}; the family is: {}",
            SUBCOMMANDS.join(", ")
        ))),
    }
}
