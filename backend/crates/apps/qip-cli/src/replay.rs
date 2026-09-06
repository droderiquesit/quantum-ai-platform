//! `qip replay --journal <path>` — does the platform this configuration
//! assembles hold anything the journal does not say?
//!
//! The event log is the platform's evidence, and evidence is only evidence
//! if something checks it. This command is that something. It verifies the
//! hash chain, rebuilds three registries *from the log alone* —
//! [`Platform::replay_eligibility`], [`Platform::replay_registrations`] and
//! [`qip_capital_fabric::replay::replay`] — and compares each against the
//! platform assembled from the same configuration. Three answers, printed
//! one per line, and a zero exit only when all three agree and the chain
//! holds.
//!
//! # What it refuses to do
//!
//! **It does not write to the journal.** Assembling a [`Platform`] on a
//! file-backed log appends to that file — the assembly record, and every
//! registration and eligibility decision the configuration commits — which
//! is exactly right for a platform resuming its own log and exactly wrong
//! for a tool asked to check one. So the journal is read where it lies, and
//! the platform is assembled on a copy that is deleted on the way out. The
//! practical consequence is that an archived journal on a read-only mount
//! can be checked at all.
//!
//! **It does not treat an absent journal as an empty one.**
//! [`qip_events::log::EventLog::open`] creates the parent directory of a
//! path that does not exist and returns a log with no records, and every
//! comparison below would then pass against nothing. A missing path, a
//! path that is not a file, an unreadable line and a file holding no
//! records are each refused by name.
//!
//! # Why the divergence is reported by record position
//!
//! "The registries differ" is a fact nobody can act on. The position is the
//! record an auditor opens: the journal's *n*th line, carrying the sequence
//! the chain committed it under, is where the platform stopped agreeing
//! with its own evidence. A registry keeps the last write per key, so an
//! earlier record the log itself later overwrote is not a divergence and is
//! skipped — reporting it would send the reader to a line that was
//! superseded on purpose.

use qip_core::error::{Error, Result};
use qip_core::{Clock, Context};
use qip_events::EventBody;
use qip_events::log::{EventLog, LogRecord};
use qip_financial::universe::Universe;
use qip_kernel::PlatformConfig;
use qip_kernel::config::EventLogDestination;
use qip_kernel::platform::{EligibilityEntry, Platform, RegistrationEntry};
use qip_observability::Telemetry;
use qip_risk::limits::LimitSet;
use qip_streaming::StreamEnvelope;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The chain holds and every registry the log rebuilds is the one the
/// platform is acting on.
pub const AGREES: u8 = 0;

/// The chain is broken, or the platform holds something the journal does
/// not. Three rather than one for the reason
/// [`crate::registrations::PENDING`] is: one is what this command exits
/// with when it could not look at all, and a caller that could not tell
/// "unreadable journal" from "the platform is acting on an unrecorded
/// registration" would report the wrong incident.
pub const DIFFERS: u8 = 3;

/// The verdict on each registry, verbatim, so a caller matching on the
/// word does not have to know how the line is laid out.
pub const IDENTICAL: &str = "identical";

/// What the command printed and what it exits with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub lines: Vec<String>,
    pub code: u8,
}

/// Check `journal` against the platform `config` assembles.
///
/// Errors — as opposed to a [`DIFFERS`] verdict — are reserved for the
/// cases where the question could not be asked: no journal, an unreadable
/// one, a platform that does not assemble. Each names the path.
pub fn verify(journal: &Path, config: PlatformConfig, clock: Arc<dyn Clock>) -> Result<Verdict> {
    let log = open(journal)?;
    let mut lines = vec![labelled(
        "journal",
        &format!(
            "{} — {} record(s), read where it lies; the platform is assembled on a copy",
            journal.display(),
            log.len()
        ),
    )];

    // The chain first, and nothing after it if it is broken. A registry
    // rebuilt from records whose hashes no longer commit to their contents
    // would be a comparison against something the log does not actually
    // say, and it would print as though it had been checked.
    if let Err(sequence) = log.verify_chain() {
        lines.push(labelled("chain", &broken(&log, sequence)));
        for registry in ["eligibility", "registrations", "fabric"] {
            lines.push(labelled(
                registry,
                "not compared — a registry rebuilt from a broken chain is not evidence",
            ));
        }
        lines.push(String::new());
        lines.push(
            "the journal has been altered since it was written; nothing here says what the \
             platform did."
                .to_string(),
        );
        return Ok(Verdict {
            lines,
            code: DIFFERS,
        });
    }
    lines.push(labelled("chain", "intact"));

    let scratch = Scratch::of(journal)?;
    let platform = assemble(config, scratch.path(), clock)?;

    let verdicts = [
        ("eligibility", eligibility(&platform)),
        ("registrations", registrations(&platform)),
        ("fabric", fabric(&platform)),
    ];
    let differing: Vec<&str> = verdicts
        .iter()
        .filter(|(_, verdict)| verdict != IDENTICAL)
        .map(|(registry, _)| *registry)
        .collect();
    for (registry, verdict) in &verdicts {
        lines.push(labelled(registry, verdict));
    }

    lines.push(String::new());
    lines.push(match differing.is_empty() {
        true => format!(
            "the platform this configuration assembles holds exactly what the journal records, \
             across all {} registry/registries.",
            verdicts.len()
        ),
        false => format!(
            "{} registry/registries disagree with the journal: {}. The platform is acting on \
             something its own log does not hold.",
            differing.len(),
            differing.join(", ")
        ),
    });
    Ok(Verdict {
        lines,
        code: match differing.is_empty() {
            true => AGREES,
            false => DIFFERS,
        },
    })
}

/// One output line: a fixed-width label so the verdicts sit in a column, and
/// so a caller can split on the first colon and compare the rest exactly.
fn labelled(label: &str, verdict: &str) -> String {
    format!("{:<15}{verdict}", format!("{label}:"))
}

/// Open the journal, refusing by name everything that is not one.
///
/// The empty-file arm is the one worth keeping: a zero-record log verifies
/// its chain, rebuilds three empty registries and matches a freshly
/// assembled platform on all three. It would exit zero having proved
/// nothing, which is the most expensive kind of green.
fn open(journal: &Path) -> Result<EventLog> {
    if !journal.exists() {
        return Err(Error::not_found(format!(
            "no journal at {}; `qip replay --journal <path>` reads an event log this platform \
             wrote and will not create one",
            journal.display()
        )));
    }
    if !journal.is_file() {
        return Err(Error::invalid(format!(
            "{} is not a file; --journal names the JSONL event log itself, not the directory \
             holding it",
            journal.display()
        )));
    }
    let log = EventLog::open(journal).map_err(|error| {
        Error::schema(format!(
            "{} is not an event log this platform wrote: {}",
            journal.display(),
            error.message()
        ))
    })?;
    if log.is_empty() {
        return Err(Error::invalid(format!(
            "{} holds no records, so there is nothing to replay and nothing this command could \
             prove by exiting zero; point --journal at the log a platform actually wrote",
            journal.display()
        )));
    }
    Ok(log)
}

/// The chain's first broken link, as the position an auditor opens.
///
/// [`EventLog::verify_chain`] answers with the sequence, which is what the
/// chain committed under; the position is which line of the file that is.
/// They differ whenever a log was resumed or a record was removed, and the
/// removal is exactly the case this command exists to catch, so both are
/// printed.
fn broken(log: &EventLog, sequence: u64) -> String {
    match log
        .records()
        .iter()
        .position(|record| record.sequence == sequence)
    {
        Some(index) => format!(
            "BROKEN at record position {} (sequence {sequence}); a record at or before it was \
             altered or removed",
            index + 1
        ),
        // Unreachable through `verify_chain`, which answers with the
        // sequence of a record it just read. Reported rather than assumed
        // away: a wrong position sends an auditor to the wrong line.
        None => format!(
            "BROKEN at sequence {sequence}, which no record in the file carries; the file and \
             the chain disagree about what is in it"
        ),
    }
}

/// The platform, assembled on the copy.
fn assemble(config: PlatformConfig, journal: &Path, clock: Arc<dyn Clock>) -> Result<Platform> {
    let context = Context::new(clock, config.seed);
    // `--journal` names the log to check, so it replaces whatever
    // destination the configuration carries. Everything else the
    // configuration says — the committed registrations and eligibility
    // decisions, which are the very things being compared — is used as
    // written.
    let config = PlatformConfig {
        event_log: EventLogDestination::file(journal),
        ..config
    };
    Platform::new(
        config,
        context,
        Telemetry::silent(),
        Universe::new(),
        LimitSet::conservative_default(),
    )
}

/// A copy of the journal, deleted when this value is dropped.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn of(journal: &Path) -> Result<Self> {
        let path = std::env::temp_dir().join(format!("qip-replay-{}.jsonl", std::process::id()));
        std::fs::copy(journal, &path).map_err(|error| {
            Error::io(format!(
                "the journal at {} could not be copied to {} to be replayed: {error}",
                journal.display(),
                path.display()
            ))
        })?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // A failure here leaves a file in the temporary directory and
        // nothing else: the copy is never read again, and refusing to exit
        // over it would turn a tidy-up into an outage.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The eligibility registry the log rebuilds, against the one the platform
/// admits users on.
fn eligibility(platform: &Platform) -> String {
    let live = platform.user_ledger().eligibility();
    match platform.replay_eligibility() {
        Err(refused) => refused_by_the_log(&refused),
        Ok(replayed) if &replayed == live => IDENTICAL.to_string(),
        Ok(_) => {
            let logged = logged::<EligibilityEntry>(platform.event_log().records());
            let last = last_write(&logged, |entry| entry.record.user.clone());
            for (index, entry) in logged.iter().enumerate() {
                if last.get(&entry.body.record.user) != Some(&index) {
                    continue;
                }
                if live.record(&entry.body.record.user) != Some(&entry.body.record) {
                    return diverged(
                        entry,
                        &format!(
                            "the journal records a decision about {} that the platform this \
                             configuration assembles does not hold",
                            entry.body.record.user.as_str()
                        ),
                    );
                }
            }
            unrecorded("an eligibility decision")
        }
    }
}

/// The registration registry the log rebuilds, against the one the
/// platform's admission gate reads.
fn registrations(platform: &Platform) -> String {
    let live = platform.registrations();
    match platform.replay_registrations() {
        Err(refused) => refused_by_the_log(&refused),
        Ok(replayed) if &replayed == live => IDENTICAL.to_string(),
        Ok(_) => {
            let logged = logged::<RegistrationEntry>(platform.event_log().records());
            let last = last_write(&logged, |entry| entry.record.source_id().to_string());
            for (index, entry) in logged.iter().enumerate() {
                let source = entry.body.record.source_id().to_string();
                if last.get(&source) != Some(&index) {
                    continue;
                }
                if live.record(&source) != Some(&entry.body.record) {
                    return diverged(
                        entry,
                        &format!(
                            "the journal records `{source}` registered by {}, and the platform \
                             this configuration assembles does not hold that registration",
                            entry.body.record.operator()
                        ),
                    );
                }
            }
            unrecorded("a venue registration")
        }
    }
}

/// The fabric state the log rebuilds, against the journal the platform
/// resumed.
///
/// Two comparisons rather than one, because the state and the count are
/// different claims: a state that matches while the counts do not means the
/// log holds fabric records the journal did not decide, which is a state
/// reached by a route the control does not own.
/// [`qip_capital_fabric::replay::replay`] re-executes every command and
/// checks the recorded outcome against the recomputed one, so its refusal
/// already names the position and is passed through as written.
fn fabric(platform: &Platform) -> String {
    match qip_capital_fabric::replay::replay(platform.event_log().records()) {
        Err(refused) => refused_by_the_log(&refused),
        Ok(replayed)
            if &replayed.state == platform.fabric_state()
                && replayed.applied == platform.fabric_records() =>
        {
            IDENTICAL.to_string()
        }
        Ok(replayed) => format!(
            "DIVERGED — the journal replays {} fabric record(s) into a state the platform's own \
             journal of {} record(s) does not hold",
            replayed.applied,
            platform.fabric_records()
        ),
    }
}

/// Every record of one body type in the log, oldest first, with the
/// position and sequence it sits at.
///
/// Read through [`StreamEnvelope::from_frame`] and
/// [`qip_streaming::StreamEnvelope::decode`] — the same two steps the
/// kernel's own replay takes — rather than by matching the producer string,
/// which is private to the kernel and would silently match nothing if it
/// changed. A record of another kind fails one of the two steps and is
/// skipped.
fn logged<T: EventBody>(records: &[LogRecord]) -> Vec<Positioned<T>> {
    let mut found = Vec::new();
    for (index, record) in records.iter().enumerate() {
        let Ok(envelope) = StreamEnvelope::from_frame(&record.event) else {
            continue;
        };
        let Ok(decoded) = envelope.decode::<T>() else {
            continue;
        };
        found.push(Positioned {
            position: index + 1,
            sequence: record.sequence,
            body: decoded.body,
        });
    }
    found
}

/// One decoded record and where in the log it sits.
struct Positioned<T> {
    position: usize,
    sequence: u64,
    body: T,
}

/// The index, within `logged`, of the last record for each key.
///
/// The registries keep the last write per key. Without this, a source
/// registered twice would report the first record as a divergence, sending
/// an auditor to a line the log itself superseded.
fn last_write<T, K, F>(logged: &[Positioned<T>], key: F) -> BTreeMap<K, usize>
where
    K: Ord,
    F: Fn(&T) -> K,
{
    let mut last = BTreeMap::new();
    for (index, entry) in logged.iter().enumerate() {
        last.insert(key(&entry.body), index);
    }
    last
}

fn diverged<T>(entry: &Positioned<T>, detail: &str) -> String {
    format!(
        "DIVERGED at record position {} (sequence {}) — {detail}",
        entry.position, entry.sequence
    )
}

/// The registries differ and every record the log holds is one the platform
/// holds too, so the platform holds something extra.
fn unrecorded(what: &str) -> String {
    format!(
        "DIVERGED — the platform holds {what} this journal never recorded; every record the \
         journal does hold, the platform holds"
    )
}

/// The log cannot be rebuilt into a registry at all.
///
/// Distinct from a divergence on purpose: a divergence is two states that
/// disagree, and this is a log that does not describe a state. Reported as
/// the refusal's own words, which name the record.
fn refused_by_the_log(refused: &Error) -> String {
    format!(
        "REFUSED — the journal does not replay: {}",
        refused.message()
    )
}
