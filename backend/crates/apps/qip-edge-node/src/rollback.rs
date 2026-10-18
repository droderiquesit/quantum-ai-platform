//! The operator's rollback wire: a request to return the cell to the policy
//! payload it served before the current one, honoured without a fetch
//! (REFLEX-8071).
//!
//! [`Cell::roll_back_policy`] keeps the previous verified payload resident so
//! a bad one is undone by an assignment rather than by waiting for the centre
//! to re-ship; until this module nothing in the deployed node could ask for
//! it, so the resident payload was a rollback nobody could perform.
//!
//! The wire is a file beside the halt flag's shape: an absolute path, named by
//! [`ROLLBACK_VARIABLE`], whose content is the decimal sequence the operator
//! wants to roll *away from*. Naming the sequence rather than writing a bare
//! "rollback" is what makes the request idempotent and safe to leave in
//! place: it acts at most once, and only while the cell is serving exactly
//! that sequence. A request left on disk after the centre ships a newer
//! payload names a sequence the cell no longer serves and does nothing, so a
//! stale file can never undo a correction it predates.
//!
//! Failure to read is not a request. A halt flag fails closed by halting; a
//! rollback is an action that changes the payload decisions are made under,
//! so its restrictive reading is "do nothing". Whether a rollback may happen
//! at all stays the cell's decision — it refuses to release a centre halt, to
//! return to a halt, or to serve a payload past its validity window — and
//! this module adds no way around any of those.
//!
//! The file is read on a thread of its own, as the halt flag is
//! ([`crate::control::Poller`]): a rollback file on a hung mount stops that
//! thread and never the decision thread, which only takes the last reading
//! published.

use qip_core::Timestamp;
use qip_core::error::{Error, Result};
use qip_edge::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration as StdDuration;

/// The environment variable naming the rollback request's path. Unset is a
/// node with no rollback control, which is how every node ran before it.
pub const ROLLBACK_VARIABLE: &str = "QIP_POLICY_ROLLBACK_PATH";

/// The request file's location, validated once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RollbackRequest {
    path: PathBuf,
}

/// What the request file says now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RollbackReading {
    /// No file: no rollback is requested.
    Absent,
    /// Roll away from this sequence, if the cell is serving it.
    AwayFrom(u64),
    /// The file exists and cannot be read as one sequence. Never acted on.
    Unreadable(String),
}

impl RollbackRequest {
    /// Name the request file.
    ///
    /// Refuses an empty or relative path, for the halt flag's reason: a
    /// relative path names a different file under every working directory,
    /// and one of them would be a control nobody could ever reach.
    pub fn at(path: impl Into<PathBuf>) -> Result<Self> {
        let path: PathBuf = path.into();
        if path.as_os_str().is_empty() {
            return Err(Error::invalid(format!(
                "configuration: {ROLLBACK_VARIABLE} is empty; name an absolute path, or leave \
                 it unset to run with no rollback control"
            )));
        }
        if !path.is_absolute() {
            return Err(Error::invalid(format!(
                "configuration: {ROLLBACK_VARIABLE} is {}, a relative path, and would name a \
                 different file under every working directory; name an absolute one",
                path.display()
            )));
        }
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the request as it stands now.
    pub fn read(&self) -> RollbackReading {
        match std::fs::read(&self.path) {
            Ok(bytes) => parse(&bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => RollbackReading::Absent,
            Err(error) => {
                RollbackReading::Unreadable(format!("cannot read {}: {error}", self.path.display()))
            }
        }
    }
}

/// One decimal sequence, surrounding whitespace allowed and nothing else. A
/// file holding anything else is refused rather than read for the digits it
/// happens to contain: `"12 then 13"` is not a request for either.
fn parse(bytes: &[u8]) -> RollbackReading {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return RollbackReading::Unreadable("the request is not UTF-8".to_string());
    };
    let trimmed = text.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return RollbackReading::Unreadable(format!(
            "the request {trimmed:?} is not one decimal sequence number"
        ));
    }
    match trimmed.parse::<u64>() {
        Ok(sequence) => RollbackReading::AwayFrom(sequence),
        Err(error) => RollbackReading::Unreadable(format!("the request {trimmed:?}: {error}")),
    }
}

/// What applying the wire did on one turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RollbackOutcome {
    /// The cell now serves `to`, having left `from`.
    RolledBack { from: u64, to: u64 },
    /// The cell refused to roll away from `from`: `refusal` is the class
    /// the node logs, `reason` the cell's own words, kept for a caller that
    /// records it and never written to stderr (see [`RollbackRefusal`]).
    Refused {
        from: u64,
        refusal: RollbackRefusal,
        reason: String,
    },
}

/// Why the cell refused a rollback, as a closed set the node may log.
///
/// The decision loop logged the cell's refusal message itself, and CodeQL's
/// rust/cleartext-logging traced that message, a string built from the
/// held payload, into stderr (main's SAST, ci run 1044). A log line drawn
/// from this enum carries only fixed text and the sequence number, and the
/// operator can still tell the four refusals apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollbackRefusal {
    /// No previous payload is held.
    NothingHeld,
    /// The centre has halted the cell; a rollback cannot release a halt.
    CentreHalted,
    /// The previous payload is itself a halt.
    PreviousIsHalt,
    /// The previous payload is outside its validity window.
    PreviousExpired,
    /// A refusal the node does not classify; the journal holds the cell's
    /// account.
    Other,
}

impl RollbackRefusal {
    /// Classify the cell's refusal by its error class and fixed opening.
    fn of(error: &qip_core::error::Error) -> Self {
        let message = error.message();
        match error.code() {
            "invalid" if message.starts_with("no previous policy payload") => Self::NothingHeld,
            "denied" if message.starts_with("the cell is halted by the centre") => {
                Self::CentreHalted
            }
            "denied" if message.starts_with("the previous payload is a halt") => {
                Self::PreviousIsHalt
            }
            "denied" if message.starts_with("the previous payload (sequence") => {
                Self::PreviousExpired
            }
            _ => Self::Other,
        }
    }

    /// The fixed text the node logs for this refusal.
    pub const fn describe(self) -> &'static str {
        match self {
            Self::NothingHeld => {
                "no previous payload is held; the centre must ship a corrected one"
            }
            Self::CentreHalted => "the centre has halted the cell and a rollback cannot release it",
            Self::PreviousIsHalt => "the previous payload is a halt",
            Self::PreviousExpired => "the previous payload is outside its validity window",
            Self::Other => "the cell refused for another reason; its journal holds the detail",
        }
    }
}

/// The request file, read on its own thread, and the decision thread's
/// memory of which request it has already acted on.
#[derive(Debug)]
pub struct RollbackWire {
    request: RollbackRequest,
    latest: Arc<Mutex<Option<RollbackReading>>>,
    stop: Arc<AtomicBool>,
    acted_on: Option<u64>,
}

impl RollbackWire {
    /// Start reading `request` every `interval`. Refuses a zero interval,
    /// which would be a busy loop on the request's mount.
    pub fn spawn(request: RollbackRequest, interval: StdDuration) -> Result<Self> {
        if interval.is_zero() {
            return Err(Error::invalid(
                "a rollback reader interval of zero is a busy loop on the request's mount; name \
                 a period",
            ));
        }
        let latest: Arc<Mutex<Option<RollbackReading>>> = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let reader = request.clone();
        let slot = Arc::clone(&latest);
        let halt = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("qip-rollback-reader".to_string())
            .spawn(move || {
                while !halt.load(Ordering::Acquire) {
                    let reading = reader.read();
                    let Ok(mut guard) = slot.lock() else {
                        return;
                    };
                    *guard = Some(reading);
                    drop(guard);
                    std::thread::sleep(interval);
                }
            })
            .map_err(|error| {
                Error::io(format!("cannot start the rollback reader thread: {error}"))
            })?;
        Ok(Self {
            request,
            latest,
            stop,
            acted_on: None,
        })
    }

    pub fn request(&self) -> &RollbackRequest {
        &self.request
    }

    /// The last reading the reader published, or `None` before the first one
    /// or after the reader panicked. Opens no file.
    pub fn latest(&self) -> Option<RollbackReading> {
        self.latest.lock().ok().and_then(|guard| guard.clone())
    }

    /// Act on the last published request, at most once per sequence and
    /// only while the cell serves that sequence. Opens no file.
    ///
    /// `None` when there was nothing to do: no request, an unreadable one, a
    /// request for a sequence the cell is not serving, or one already acted
    /// on — including one the cell refused, so a refusal is reported once
    /// and not on every turn while the file stays in place.
    pub fn apply(&mut self, cell: &mut Cell, now: Timestamp) -> Option<RollbackOutcome> {
        let RollbackReading::AwayFrom(from) = self.latest()? else {
            return None;
        };
        if self.acted_on == Some(from) || cell.policy_sequence() != Some(from) {
            return None;
        }
        self.acted_on = Some(from);
        Some(match cell.roll_back_policy(now) {
            Ok(to) => RollbackOutcome::RolledBack { from, to },
            Err(error) => RollbackOutcome::Refused {
                from,
                refusal: RollbackRefusal::of(&error),
                reason: error.message().to_string(),
            },
        })
    }
}

impl Drop for RollbackWire {
    /// Tells the reader to stop after its current read. Not joined, for the
    /// poller's reason: a reader blocked on a hung mount would hang the drop.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
