//! The capital-fabric declaration: the one way a destination, a corridor or
//! a transfer-gate assessment reaches this process.
//!
//! `Platform::decide_fabric` has, until this module existed, been called from
//! exactly one place in production — `stage_learn`, through
//! `reconcile_wallet` — and that caller passes only `FabricCommand::Wallet`.
//! Nothing outside tests issued a `Destination`, a `Corridor` or a `Gate`
//! command. So blueprint §38.4's destination registry, §37.1's corridor
//! lifecycle and §37.3's seven-check transfer gate were built, tested and
//! reached by nothing: `qip-api`'s `GET /transfer-gate` rendered
//! `last_assessment: null` permanently rather than transiently, which is a
//! control that reads as protection and is not.
//!
//! # Why a declaration file and not a cycle stage
//!
//! Every input the gate weighs is a fact somebody declared rather than one a
//! cycle measures: the corridor and its destination are proposed records, the
//! custody agreements and the three enforcement attestations are operator
//! statements. A stage that manufactured those in order to have something to
//! assess would be a control weighing inputs it invented — and with no
//! corridor proposed the gate would veto at check 1 on every pass, which
//! reads as a working control and is a constant. So the producer is an
//! operator route of the shape [`crate::statement`] already has: a person
//! states the facts, the kernel derives the ruling it is entitled to derive,
//! the control decides, and the record lands in the hash-chained log.
//!
//! ADR 0021 does not refuse this. Its decision table permits "Typed transfer
//! intents (§37.3) | Permitted — an intent is a record, not a movement" and
//! "A deterministic transfer gate | Permitted, and desirable: a gate that
//! refuses is the safe half". What it refuses is MPC signing corridors,
//! withdrawal APIs and live venue submission — the engine behind an approved
//! intent, never the caller in front of the gate. **An admitted verdict
//! carries no way to execute**, there is no transfer engine in the workspace,
//! and nothing in this process consumes an `Approved`.
//!
//! # The file
//!
//! `QIP_CAPITAL_FABRIC_PATH` names a JSON document holding an ordered list of
//! commands, each one a `FabricCommand` as the event log writes it — the same
//! shape a replay reads, so what an operator declares and what the chain
//! carries cannot drift apart:
//!
//! ```json
//! {
//!   "commands": [
//!     {
//!       "subject": "destination",
//!       "action": "propose",
//!       "key": { "asset": "USD", "address": "treasury-account" },
//!       "by": "treasury-desk",
//!       "at": "2026-09-05T06:00:00Z"
//!     }
//!   ]
//! }
//! ```
//!
//! Order is the operator's, and it is applied in order: a corridor naming a
//! destination the list has not yet proposed is refused by the control, and
//! that refusal is a record, because the refusal is a decision and belongs in
//! the log.
//!
//! **Every fractional number is written as a string.** A JSON number that is
//! not an exact integer is refused by position, before any command is
//! deserialised. `Decimal`'s deserialiser accepts a float and reaches
//! `Decimal::from_f64`, so `"hourly": 0.1` would record a corridor cap that
//! is not the cap the desk wrote down — the same defect the wallet statement
//! refuses, and it matters at least as much on a cap as on a balance. The
//! check walks the whole document rather than naming fields, so a field added
//! to the command types later is covered the day it is added.
//!
//! # What is applied, and once
//!
//! The composition root reads and validates the file at start, refusing
//! anything malformed, and applies every command before serving. Each
//! admitted `POST /cycle` then re-reads the file when its modification time
//! or length has moved and applies **only the commands appended since**.
//!
//! A prefix that has changed stops the process rather than being re-applied.
//! A fabric command is not a figure that gets corrected like a balance — it
//! is an act with a place in a lifecycle, and applying an edited one again
//! would either double-journal it or have the control refuse a transition
//! the log already made. The event log is hash-chained on purpose and nothing
//! may edit history that has been sealed; the declaration is the operator's
//! copy of that history, so the refusal says to append rather than amend.
//!
//! **No refusal quotes the file.** Every message names the position, the key
//! or the class and stops there. A declaration names accounts, references and
//! counterparties, and a 503 body reaches whoever can call the route, the
//! stderr of the process, and whichever ticket the line is pasted into.
//!
//! Absent variable: no declaration, the banner says so, and `/transfer-gate`
//! keeps answering `last_assessment: null` — honestly, because nothing was
//! declared.

use crate::auth::Authenticator;
use crate::http::{Handler, Method, Request, Response, StreamDecision};
use crate::json;
use crate::routes::Api;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Timestamp};
use qip_kernel::Platform;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Re-exported so the composition root and this module's tests name the
/// variable and the bound in one place. The vocabulary they describe lives in
/// the kernel, for the reason `qip_kernel::fabric_declaration` gives: this
/// layer is forbidden an edge to the fabric crate, and buying a feature by
/// deleting that boundary is not a trade this module makes.
pub use qip_kernel::fabric_declaration::{Declaration, FABRIC_PATH_VARIABLE, MAX_FABRIC_COMMANDS};

/// What the file looked like when it was last read: modification time and
/// length.
///
/// Both, because a file rewritten within the filesystem's timestamp
/// granularity keeps its modification time, and a length that also stayed the
/// same is the one edit this cannot see. Said here so nobody reads the check
/// as content-based — the prefix comparison in [`FabricFeed::refresh`] is
/// what actually holds the applied history, and it is content-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint {
    modified: Option<SystemTime>,
    len: u64,
}

impl Fingerprint {
    fn of(path: &str) -> Result<Self> {
        let metadata = std::fs::metadata(path).map_err(|error| {
            Error::io(format!(
                "{FABRIC_PATH_VARIABLE} names {path}, which cannot be read: {error}. Unset it to \
                 run with nothing declared; a named declaration that does not read is not a \
                 declaration that is absent"
            ))
        })?;
        Ok(Self {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        })
    }
}

/// The declaration file this process re-reads, what it has applied, and the
/// refusal it last gave.
#[derive(Debug)]
pub struct FabricFeed {
    path: String,
    fingerprint: Fingerprint,
    declaration: Declaration,
    /// How many of the declaration's commands have been journalled.
    ///
    /// The applied prefix, and the reason a refresh compares rather than
    /// re-applies: a command already on the chain is history, and history
    /// that has been sealed is not edited.
    applied: usize,
    /// The fingerprint of a file this feed already read and refused, with the
    /// refusal it gave.
    ///
    /// A broken file refuses every admitted cycle — that is the point, and it
    /// is not softened — but re-reading and re-parsing the same bytes to
    /// reach the same refusal spends the request's time on work whose answer
    /// is already known. The fingerprint is still taken every time, so an
    /// operator who fixes the file is picked up on the next cycle without a
    /// restart.
    refused: Option<(Fingerprint, Error)>,
}

impl FabricFeed {
    /// Open the declaration the environment names, or `None` when it names
    /// none.
    ///
    /// The environment is passed in rather than read: the composition root is
    /// the one place that may read it, and a test hands in a map.
    pub fn from_env(lookup: &dyn Fn(&str) -> Option<String>) -> Result<Option<Self>> {
        match lookup(FABRIC_PATH_VARIABLE).filter(|value| !value.trim().is_empty()) {
            None => Ok(None),
            Some(path) => Self::open(&path).map(Some),
        }
    }

    /// Read and validate the file at `path`, refusing it by position.
    pub fn open(path: &str) -> Result<Self> {
        let (fingerprint, declaration) = read(path)?;
        Ok(Self {
            path: path.to_string(),
            fingerprint,
            declaration,
            applied: 0,
            refused: None,
        })
    }

    /// The declaration as last read.
    pub fn declaration(&self) -> &Declaration {
        &self.declaration
    }

    /// Where the declaration is read from.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// How many commands have been journalled.
    pub fn applied(&self) -> usize {
        self.applied
    }

    /// Apply everything not yet applied, and record how far the feed got.
    ///
    /// The count is advanced by what was actually journalled, so a refusal
    /// part-way through leaves the feed pointing at the first command that
    /// did not land rather than at the end of the file. The next attempt
    /// resumes there instead of re-journalling what already succeeded.
    pub fn apply_pending(&mut self, platform: &mut Platform, now: Timestamp) -> Result<usize> {
        let mut applied = 0;
        let outcome = self
            .declaration
            .apply_counting(platform, self.applied, now, &mut applied);
        // Advanced on both paths, and that is the point: the kernel offers no
        // transaction, so whatever `applied` counts is already on the chain
        // whether the call ended in `Ok` or `Err`. A feed that left the count
        // alone after a refusal would re-offer those records on the next
        // attempt, and the control would refuse transitions the log had
        // already made — a burst of refusal records caused by the retry
        // rather than by anything the operator did.
        self.applied += applied;
        outcome.map(|()| applied)
    }

    /// Re-read the file when it has changed, and refuse an edited prefix.
    ///
    /// `Some` carries the number of newly appended commands, already held;
    /// `None` means the file is as it was. A file that has gone, that has
    /// changed into something the parser refuses, or whose applied prefix has
    /// been edited is an error and the held declaration is left as it was.
    pub fn refresh(&mut self) -> Result<Option<usize>> {
        let fingerprint = Fingerprint::of(&self.path)?;
        if fingerprint == self.fingerprint {
            return Ok(None);
        }
        if let Some((refused_at, error)) = &self.refused
            && *refused_at == fingerprint
        {
            return Err(error.clone());
        }
        let outcome = read(&self.path).and_then(|(fingerprint, declaration)| {
            // The applied prefix is history. A declaration that has amended
            // one of its own past commands is not a corrected figure — it is
            // a rewritten act, and the chain already carries the act it
            // rewrote. The refusal names the position and says to append.
            if declaration.len() < self.applied {
                return Err(Error::invalid(format!(
                    "the declaration now lists {} commands and {} have already been journalled; \
                     removing an applied command does not remove its record from the chain, so \
                     append rather than amend",
                    declaration.len(),
                    self.applied
                )));
            }
            if !declaration.shares_prefix_with(&self.declaration, self.applied) {
                let position = declaration
                    .first_difference_with(&self.declaration, self.applied)
                    .unwrap_or(0);
                return Err(Error::invalid(format!(
                    "commands[{position}] has changed and it has already been journalled; the \
                     event log is hash-chained and a sealed record is not edited, so append the \
                     correcting act rather than amending the one that was applied"
                )));
            }
            Ok((fingerprint, declaration))
        });
        match outcome {
            Ok((fingerprint, declaration)) => {
                let appended = declaration.len() - self.applied;
                self.fingerprint = fingerprint;
                self.declaration = declaration;
                self.refused = None;
                Ok(Some(appended))
            }
            Err(error) => {
                self.refused = Some((fingerprint, error.clone()));
                Err(error)
            }
        }
    }

    /// The start-up banner line.
    pub fn describe(&self) -> String {
        format!(
            "{} ({} command(s) declared)",
            self.path,
            self.declaration.len()
        )
    }
}

/// The banner line when no declaration is named.
pub fn absent_banner() -> String {
    format!("none ({FABRIC_PATH_VARIABLE} unset)")
}

/// Read and validate the file at `path`, with the fingerprint it had when it
/// was read.
///
/// The fingerprint is taken **before** the read, so a file rewritten between
/// the two is seen as changed on the next check rather than recorded as the
/// version that was read.
fn read(path: &str) -> Result<(Fingerprint, Declaration)> {
    let fingerprint = Fingerprint::of(path)?;
    let text = std::fs::read_to_string(path).map_err(|error| {
        Error::io(format!(
            "{FABRIC_PATH_VARIABLE} names {path}, which cannot be read: {error}"
        ))
    })?;
    let declaration = Declaration::parse(&text)?;
    Ok((fingerprint, declaration))
}

/// Applies commands appended to the declaration before an admitted cycle
/// runs.
///
/// Wraps the router the way [`crate::statement::StatementRefresh`] does, and
/// nests with it: an operator who declares a corridor while the process is
/// serving does not restart it, and the corridor is on the chain before the
/// cycle that could be measured against it.
pub struct FabricRefresh<H> {
    inner: H,
    feed: Arc<Mutex<FabricFeed>>,
    platform: Arc<Mutex<Platform>>,
    authenticator: Arc<Authenticator>,
    clock: Arc<dyn Clock>,
}

impl<H> std::fmt::Debug for FabricRefresh<H> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FabricRefresh").finish_non_exhaustive()
    }
}

impl<H: Handler> FabricRefresh<H> {
    pub fn new(
        inner: H,
        feed: Arc<Mutex<FabricFeed>>,
        platform: Arc<Mutex<Platform>>,
        authenticator: Arc<Authenticator>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            inner,
            feed,
            platform,
            authenticator,
            clock,
        }
    }

    /// Whether `request` is a `POST /cycle` the API would admit.
    fn is_admitted_cycle(&self, request: &Request) -> bool {
        let Some(route) = Api::route_for(request.method, &request.path) else {
            return false;
        };
        if route.method != Method::Post || route.pattern != "/cycle" {
            return false;
        }
        let now = self.clock.now();
        self.authenticator
            .authenticate(request.header("authorization"), now)
            .and_then(|principal| principal.require(route.required_role))
            .is_ok()
    }

    /// Re-read the file if it moved and apply what was appended; `Err` is the
    /// refusal the cycle is answered with.
    fn refresh(&self) -> Result<()> {
        let now = self.clock.now();
        let mut feed = self
            .feed
            .lock()
            .map_err(|_| Error::invalid("the fabric declaration is in an inconsistent state"))?;
        // The read is here, under the feed lock and no other. A refusal
        // returns without ever touching the platform lock, so a broken file
        // cannot make the cycle route queue behind whatever holds it.
        let Some(appended) = feed.refresh()? else {
            return Ok(());
        };
        if appended == 0 {
            return Ok(());
        }
        let mut platform = self
            .platform
            .lock()
            .map_err(|_| Error::invalid("the platform is in an inconsistent state"))?;
        feed.apply_pending(&mut platform, now).map(|_| ())
    }
}

impl<H: Handler> Handler for FabricRefresh<H> {
    fn handle(&self, request: &Request) -> Response {
        if self.is_admitted_cycle(request)
            && let Err(error) = self.refresh()
        {
            eprintln!(
                "qip-api: the capital-fabric declaration did not apply: {}",
                error.message()
            );
            return Response::json(
                503,
                format!(
                    r#"{{"error":{},"source":{}}}"#,
                    json::string(error.message()),
                    json::string(FABRIC_PATH_VARIABLE)
                ),
            );
        }
        self.inner.handle(request)
    }

    fn stream(&self, request: &Request) -> StreamDecision {
        self.inner.stream(request)
    }
}
