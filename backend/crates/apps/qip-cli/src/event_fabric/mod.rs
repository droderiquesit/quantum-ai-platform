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

pub mod admin;
pub mod grant;
pub mod inspect;
pub mod schema_gate;

use qip_core::error::{Error, Result};
use qip_core::{Clock, SystemClock};
use qip_transport::event_fabric::auth::BearerToken;
use qip_transport::event_fabric::transport::FabricTransport;
use qip_transport::retry::{Sleeper, ThreadSleeper};
use qip_transport::tls::LinkSecurity;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;

/// Every subcommand this family answers, in the order the refusal names them.
///
/// The one list [`dispatch`] matches and the unknown-subcommand refusal
/// prints, so the two cannot disagree about what exists.
pub const SUBCOMMANDS: [&str; 4] = [
    grant::SUBCOMMAND,
    admin::ISOLATE,
    admin::RELEASE,
    schema_gate::SUBCOMMAND,
];

/// How a variable is looked up: the process environment in production, a
/// map in a test.
pub type Lookup = Box<dyn Fn(&str) -> Option<String>>;

/// How a transport to a checked peer is opened, carrying the identity every
/// call presents.
pub type Connector = Box<dyn Fn(SocketAddr, BearerToken) -> Box<dyn FabricTransport + Send>>;

/// How a transport to a checked peer is opened over the link the command
/// configured (ADR 0112): mutual TLS, or the plaintext only a `local`
/// deployment may choose.
///
/// Separate from [`Connector`] because the link is decided per run from the
/// deployment, and a connector that could not be told the link would be
/// free to open plaintext under a run configured for TLS. Fallible because
/// building a TLS client from the workload identity can fail, and that
/// failure must stop the run rather than become a plaintext connection.
pub type LinkConnector =
    Box<dyn Fn(&LinkSecurity, SocketAddr, BearerToken) -> Result<Box<dyn FabricTransport + Send>>>;

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
    connect_link: LinkConnector,
    /// What this machine's DMI `product_name` says, read once by
    /// [`Self::process`] and `None` everywhere else, so a test never depends
    /// on whether the host running it is a Compute Engine guest.
    product_name: Option<String>,
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
            connect_link: Box::new(grant::link_transport),
            product_name: std::fs::read_to_string(
                qip_core::deployment::COMPUTE_ENGINE_PRODUCT_FILE,
            )
            .ok(),
        }
    }

    /// Every seam supplied by the caller except the link connector, which
    /// is the production one ([`grant::link_transport`]) until
    /// [`Self::with_link_connector`] replaces it.
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
            connect_link: Box::new(grant::link_transport),
            product_name: None,
        }
    }

    /// The same environment opening its secured links through `connect_link`
    /// — a test's recording broker, or a TLS listener it owns.
    #[must_use]
    pub fn with_link_connector(mut self, connect_link: LinkConnector) -> Self {
        self.connect_link = connect_link;
        self
    }

    pub fn variable(&self, name: &str) -> Option<String> {
        (self.lookup)(name)
    }

    /// The signs that this process runs on Cloud Run, Kubernetes or Compute
    /// Engine, which refuse `QIP_DEPLOYMENT=local` (ADR 0110 decision 5):
    /// a `local` set by an unreviewed edit on a deployed machine would
    /// otherwise re-admit the plaintext link and the shared key.
    pub fn deployment_markers(&self) -> qip_core::deployment::DeploymentMarkers {
        qip_core::deployment::DeploymentMarkers::observe(
            &|name| self.variable(name),
            self.product_name.as_deref(),
        )
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

    /// A transport to `peer` over `link`, presenting `identity`.
    pub fn connect_over(
        &self,
        link: &LinkSecurity,
        peer: SocketAddr,
        identity: BearerToken,
    ) -> Result<Box<dyn FabricTransport + Send>> {
        (self.connect_link)(link, peer, identity)
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
        action @ (admin::ISOLATE | admin::RELEASE) => {
            admin::run(action, &arguments[1..], environment)
        }
        schema_gate::SUBCOMMAND => schema_gate::run(&arguments[1..], environment),
        other => Err(Error::invalid(format!(
            "unknown event-fabric subcommand {other:?}; the family is: {}",
            SUBCOMMANDS.join(", ")
        ))),
    }
}
