//! The deep brain's operations loop: the AIOps entity graph, incident
//! timeline and runbook-only remediation given a production caller, and the
//! critical-path objectives evaluated and recorded every cycle (OBS-7115,
//! OBS-7121, OBS-7122).
//!
//! # Why here
//!
//! `qip_observability::aiops` held the graph, the timeline and a
//! `Remediator` that could only act through an `Effector` some composition
//! root implements — and no root implemented one, so every piece was tested
//! and none ran. The deep brain is the process that owns its own optional
//! load (the research loop, the search, the fit), so it is the one place an
//! automated remediation can be both real and safe: the only effect this
//! file's effector has is to switch off one of this node's own research
//! capabilities until the process restarts. It cannot reach a venue, a
//! ledger, another service or a cloud resource, and an action no runbook
//! names is refused and audited.
//!
//! # What an incident is
//!
//! A level shift in this node's own iteration time — a cycle plus everything
//! run in the gap after it — found by `detect_level_shift`, with no static
//! threshold. The incident's timeline joins it with the data anomalies,
//! model drift and venue behaviour the same window saw, so the operator
//! reads one ordered account instead of four dashboards.
//!
//! # What an incident leaves behind (ADR 0213)
//!
//! Every iteration's time is journalled as an [`IterationSample`] before the
//! detector reads it, so the series a shift was found in is a window of a
//! hash-chained log and not a buffer that died with the process. When a
//! shift opens an incident, in this order:
//!
//! 1. A [`ReplayFixture`] naming that window by journal sequence range, the
//!    platform log's sequence range and the cycles' correlation ids is built,
//!    replayed from the journal, and drafted to the operations outbox only if
//!    the replay finds the same shift. No fixture, no automated action: a
//!    regression is turned into a test before anything is done about it
//!    (R12-RES-9087).
//! 2. The diagnosis proposes actions. Only those a committed runbook names
//!    reach the [`Remediator`]; the rest — scaling, rolling back, restarting
//!    or draining anything but this node's own research — are written into
//!    the record for a person, because they need a cloud credential this
//!    process does not hold and will not be given (R12-OBS-9158).
//! 3. An [`IncidentRecord`] carrying every audit record the remediator wrote
//!    for the incident and the fixture's digest is journalled, then drafted to
//!    the outbox. Nothing is sent anywhere: the outbox is a file a person
//!    reads, and opening the incident or the pull request outside is their
//!    act through their own credential (ADR 0114 D8).

use qip_agency::outbox::OUTBOX_CHANNEL;
use qip_core::error::{Error, Result};
use qip_core::{Clock, Duration, Timestamp};
use qip_events::{Envelope, EventBody, EventLog, Topic};
use qip_kernel::Platform;
use qip_observability::aiops::{
    Anomaly, AuditRecord, AuditSink, Effector, EntityGraph, EntityKey, Remediator, Runbook, Signal,
    SignalClass, TimelineEvent, TimelineSource, Triage, build_timeline, detect_level_shift, triage,
};
use qip_observability::critical_paths;
use qip_observability::metrics::{Metrics, labels, names};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The capabilities this node may switch off, by the names runbooks target.
pub const CAPABILITIES: [&str; 1] = ["research"];
/// Iterations the level-shift detector reads either side of a candidate.
pub const SHIFT_HALF: usize = 6;
/// Iteration times retained for the detector.
pub const SERIES_RETAINED: usize = 128;
/// Timeline events retained between incidents.
pub const TIMELINE_RETAINED: usize = 256;
/// Signals the graph keeps per entity.
pub const GRAPH_RETENTION: usize = 64;
/// A fill miss past this many basis points is venue behaviour worth a line
/// on an incident's timeline. The research loop's residual threshold.
pub const VENUE_BEHAVIOUR_BPS: f64 = crate::research::RESIDUAL_BPS;

/// `Solver::Quantum.as_str()`, as journalled in `SolverRoutingJournal::ran`.
pub const QUANTUM_SOLVER: &str = "quantum";

/// The pre-approved runbooks. Committed code, reviewed like any other: an
/// automated action exists only if a line here names it.
pub fn runbooks() -> Vec<Runbook> {
    vec![Runbook {
        name: "shed-research-on-iteration-shift".to_string(),
        action: "disable_capability".to_string(),
        targets: BTreeSet::from(["research".to_string()]),
    }]
}

/// The producer every remediation record carries.
pub const REMEDIATION_ORIGIN: &str = "deepbrain/aiops";

/// The producer every journalled iteration sample carries.
pub const SAMPLE_ORIGIN: &str = "deepbrain/aiops-sample";

/// The producer every journalled incident record carries.
pub const INCIDENT_ORIGIN: &str = "deepbrain/aiops-incident";

/// The producer every draft in the operations outbox carries.
pub const OPERATIONS_OUTBOX_ORIGIN: &str = "deepbrain/aiops-outbox";

/// The series the level-shift detector reads.
pub const ITERATION_SERIES: &str = "qip_deepbrain_iteration_ms";

/// How far before a shift an incident's timeline, and the diagnosis's look
/// for a recent promotion, reach. The timeline's existing lookback, named so
/// the two cannot drift apart; not a new threshold.
pub const INCIDENT_LOOKBACK: Duration = Duration::from_hours(1);

/// Why a proposed action that no runbook names is left for a person.
pub const FOR_A_PERSON: &str = "no runbook names it, and carrying it out needs a cloud \
     credential this process does not hold and will not be given (ADR 0213 decision 2); a \
     person reads the incident and acts through their own";

/// The node's runbook for `action` on `target`, if one names both.
pub fn runbook_for(action: &str, target: &str) -> Option<String> {
    runbooks()
        .into_iter()
        .find(|r| r.action == action && r.targets.contains(target))
        .map(|r| r.name)
}

/// One iteration's time as the detector will read it, journalled first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IterationSample {
    pub at: Timestamp,
    pub cycle: u64,
    pub iteration_ms: f64,
    /// The platform log's last sequence when the sample was taken.
    pub platform_sequence: Option<u64>,
    /// The cycle's correlation id, where its journal entry was found.
    pub correlation_id: Option<String>,
}

impl EventBody for IterationSample {
    const TOPIC: Topic = Topic::SystemAlert;
    const SCHEMA_VERSION: u32 = 1;
}

/// The regression an incident is, as a window of the operations journal that
/// replays to the same shift: a draft for a person to commit as a test.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReplayFixture {
    pub incident: String,
    pub series: String,
    pub half: usize,
    /// First and last journal sequence of the window, inclusive.
    pub journal_sequences: (u64, u64),
    /// Samples the window holds; a replay finding any other count refuses.
    pub samples: usize,
    /// SHA-256 over the window's record hashes in order, so the fixture names
    /// these records and not others that happen to share their sequences.
    pub window_digest: String,
    /// The platform log's sequences the window spans, where it had any.
    pub platform_sequences: Option<(u64, u64)>,
    /// The cycles' correlation ids, to join the platform log.
    pub correlation_ids: BTreeSet<String>,
    /// What the window must replay to.
    pub expected: Anomaly,
    /// The test a person commits, in words.
    pub proposed_test: String,
}

/// An incident's link to its drafted fixture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixtureLink {
    /// The outbox record's payload hash: the digest of the fixture as drafted.
    pub digest: String,
    pub outbox_sequence: u64,
}

/// An action the diagnosis proposed that no runbook names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HumanAction {
    pub action: String,
    pub target: String,
    pub why: String,
}

/// The incident as journalled and drafted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IncidentRecord {
    pub incident: String,
    pub opened_at: Timestamp,
    pub shift: Anomaly,
    pub timeline: Vec<TimelineEvent>,
    /// Every audit record the remediator wrote for this incident, in order.
    pub actions: Vec<AuditRecord>,
    /// Proposed actions left for a person, with why.
    pub for_a_person: Vec<HumanAction>,
    /// The drafted fixture, or why none could be drafted.
    pub fixture: std::result::Result<FixtureLink, String>,
    /// Where the record was drafted: never anywhere a message leaves from.
    pub channel: String,
    /// Severity, owner and why (R12-OBS-1172). `None` only on a record
    /// journalled before triage existed, which still decodes.
    #[serde(default)]
    pub triage: Option<Triage>,
}

impl EventBody for IncidentRecord {
    const TOPIC: Topic = Topic::SystemAlert;
    const SCHEMA_VERSION: u32 = 1;
}

/// A draft in the operations outbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OperationsDraft {
    Fixture(ReplayFixture),
    Incident(IncidentRecord),
}

impl EventBody for OperationsDraft {
    const TOPIC: Topic = Topic::SystemAlert;
    const SCHEMA_VERSION: u32 = 1;
}

/// A draft as it sits in the outbox, with its digest.
#[derive(Debug, Clone, PartialEq)]
pub struct DraftRecord {
    pub sequence: u64,
    pub digest: String,
    pub draft: OperationsDraft,
}

/// SHA-256 over the record hashes of a window, in order.
fn window_digest<'a>(hashes: impl Iterator<Item = &'a str>) -> String {
    let mut joined = String::new();
    for hash in hashes {
        joined.push_str(hash);
        joined.push('\n');
    }
    qip_core::sha256_hex(joined.as_bytes())
}

/// Replay `fixture`'s window from `journal` through the detector.
///
/// Refuses a window whose sample count or record hashes are not the ones the
/// fixture names, rather than replaying different records under its name.
pub fn replay_fixture(
    journal: &RemediationJournal,
    fixture: &ReplayFixture,
) -> Result<Option<Anomaly>> {
    let (from, to) = fixture.journal_sequences;
    let window: Vec<(u64, String, IterationSample)> = journal
        .samples()?
        .into_iter()
        .filter(|(sequence, _, _)| (from..=to).contains(sequence))
        .collect();
    if window.len() != fixture.samples {
        return Err(Error::invalid(format!(
            "the fixture names {} samples in journal sequences {from}..={to} and the journal \
             holds {}; replay it against the journal it was drafted from",
            fixture.samples,
            window.len()
        )));
    }
    if window_digest(window.iter().map(|(_, hash, _)| hash.as_str())) != fixture.window_digest {
        return Err(Error::invalid(
            "the journal's records in the fixture's window are not the ones it was drafted from",
        ));
    }
    let points: Vec<(Timestamp, f64)> = window
        .iter()
        .map(|(_, _, sample)| (sample.at, sample.iteration_ms))
        .collect();
    detect_level_shift(&fixture.series, &points, fixture.half)
}

/// One remediation audit record as it sits on the journal (R12-CICD-9081).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemediationEntry {
    pub record: AuditRecord,
}

impl EventBody for RemediationEntry {
    /// An automated change to the running node is a System fact; the journal
    /// is its own log, as the ambient journal's is, so the kernel's replay
    /// never decodes one as its own type.
    const TOPIC: Topic = Topic::SystemAlert;
    const SCHEMA_VERSION: u32 = 1;
}

/// The hash-chained log a remediation's audit record is written to *before*
/// the effector runs. Cloning shares the log, so the composition root can
/// keep a handle to read it back while the [`Remediator`] owns the sink.
#[derive(Debug, Clone)]
pub struct RemediationJournal {
    log: Arc<Mutex<EventLog>>,
    /// The operations outbox (ADR 0213): incident and fixture drafts for a
    /// person, on a chain of its own. Not the agency's outbox file, which the
    /// agency door alone appends to; two writers on one chain is a chain
    /// neither can vouch for.
    outbox: Arc<Mutex<EventLog>>,
    clock: Arc<dyn Clock>,
}

fn open_verified(path: Option<PathBuf>, what: &str) -> Result<EventLog> {
    let log = match path {
        Some(path) => EventLog::open(path)?,
        None => EventLog::in_memory(),
    };
    if let Err(sequence) = log.verify_chain() {
        return Err(Error::invalid(format!(
            "the {what}'s hash chain breaks at sequence {sequence}; move the file aside rather \
             than extend it"
        )));
    }
    Ok(log)
}

fn lock(log: &Mutex<EventLog>) -> Result<std::sync::MutexGuard<'_, EventLog>> {
    log.lock()
        .map_err(|_| Error::io("an operations log's lock was poisoned"))
}

fn suffixed(path: &std::path::Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

impl RemediationJournal {
    /// Open the journal at `path`, or in memory, refusing a chain that does
    /// not verify: appending to an edited journal extends a record nobody can
    /// trust. The outbox sits at `path` with `.outbox.jsonl` appended.
    pub fn open(path: Option<PathBuf>, clock: Arc<dyn Clock>) -> Result<Self> {
        let outbox = path.as_deref().map(|p| suffixed(p, ".outbox.jsonl"));
        Self::open_pair(path, outbox, clock)
    }

    /// Open the journal and the outbox at paths of their own.
    pub fn open_pair(
        journal: Option<PathBuf>,
        outbox: Option<PathBuf>,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        Ok(Self {
            log: Arc::new(Mutex::new(open_verified(journal, "remediation journal")?)),
            outbox: Arc::new(Mutex::new(open_verified(outbox, "operations outbox")?)),
            clock,
        })
    }

    /// Beside the platform's own log when that is a file, in memory when it
    /// is, so the journal is as durable as the decisions it accompanies.
    pub fn beside(
        platform_log: &qip_kernel::EventLogDestination,
        clock: Arc<dyn Clock>,
    ) -> Result<Self> {
        match platform_log {
            qip_kernel::EventLogDestination::InMemory => Self::open_pair(None, None, clock),
            qip_kernel::EventLogDestination::File { path, .. } => Self::open_pair(
                Some(suffixed(path, ".aiops.jsonl")),
                Some(suffixed(path, ".aiops-outbox.jsonl")),
                clock,
            ),
        }
    }

    fn append_to<T: EventBody>(
        log: &Mutex<EventLog>,
        prefix: &str,
        producer: &str,
        correlation: &str,
        at: Timestamp,
        body: T,
    ) -> Result<(u64, String)> {
        let mut log = lock(log)?;
        let next = log.len() as u64 + 1;
        let event = Envelope::new(
            qip_core::EventId::from_string(format!("{prefix}-{next}")),
            at,
            at,
            qip_core::lineage::Lineage::root(
                qip_core::CorrelationId::from_string(correlation.to_string()),
                producer,
            ),
            body,
        )
        .erase()?;
        let digest = event.payload_hash.clone();
        let sequence = log.append(&event)?;
        Ok((sequence, digest))
    }

    fn journal_entry<T: EventBody>(
        &self,
        producer: &str,
        correlation: &str,
        at: Timestamp,
        body: T,
    ) -> Result<u64> {
        Self::append_to(&self.log, "evt-aiops", producer, correlation, at, body).map(|(s, _)| s)
    }

    /// Append a draft to the outbox and return its link. The append is the
    /// outbox's whole effect; nothing in the platform reads it but a person.
    fn draft(
        &self,
        correlation: &str,
        at: Timestamp,
        draft: OperationsDraft,
    ) -> Result<FixtureLink> {
        let (outbox_sequence, digest) = Self::append_to(
            &self.outbox,
            "evt-aiops-outbox",
            OPERATIONS_OUTBOX_ORIGIN,
            correlation,
            at,
            draft,
        )?;
        Ok(FixtureLink {
            digest,
            outbox_sequence,
        })
    }

    /// Every incident record journalled, in order.
    pub fn incidents(&self) -> Result<Vec<IncidentRecord>> {
        let log = lock(&self.log)?;
        log.events()
            .filter(|e| e.lineage.producer == INCIDENT_ORIGIN)
            .map(|e| Ok(e.decode::<IncidentRecord>()?.body))
            .collect()
    }

    /// Every journalled iteration sample with its sequence and record hash.
    pub fn samples(&self) -> Result<Vec<(u64, String, IterationSample)>> {
        let log = lock(&self.log)?;
        log.records()
            .iter()
            .filter(|r| r.event.lineage.producer == SAMPLE_ORIGIN)
            .map(|r| {
                Ok((
                    r.sequence,
                    r.record_hash.clone(),
                    r.event.decode::<IterationSample>()?.body,
                ))
            })
            .collect()
    }

    /// Every outbox draft, in order, with its digest.
    pub fn drafts(&self) -> Result<Vec<DraftRecord>> {
        let outbox = lock(&self.outbox)?;
        outbox
            .records()
            .iter()
            .filter(|r| r.event.lineage.producer == OPERATIONS_OUTBOX_ORIGIN)
            .map(|r| {
                Ok(DraftRecord {
                    sequence: r.sequence,
                    digest: r.event.payload_hash.clone(),
                    draft: r.event.decode::<OperationsDraft>()?.body,
                })
            })
            .collect()
    }

    /// The fixture `link` names, found by digest and sequence, or `None`.
    pub fn linked_fixture(&self, link: &FixtureLink) -> Result<Option<ReplayFixture>> {
        Ok(self.drafts()?.into_iter().find_map(|d| match d.draft {
            OperationsDraft::Fixture(fixture)
                if d.digest == link.digest && d.sequence == link.outbox_sequence =>
            {
                Some(fixture)
            }
            _ => None,
        }))
    }

    /// Whether both chains still verify.
    pub fn verify(&self) -> Result<()> {
        for (what, log) in [("journal", &self.log), ("outbox", &self.outbox)] {
            if let Err(sequence) = lock(log)?.verify_chain() {
                return Err(Error::invalid(format!(
                    "the operations {what}'s chain breaks at sequence {sequence}"
                )));
            }
        }
        Ok(())
    }
    /// Every record journalled so far, in order.
    pub fn records(&self) -> Result<Vec<AuditRecord>> {
        let log = lock(&self.log)?;
        log.events()
            .filter(|e| e.lineage.producer == REMEDIATION_ORIGIN)
            .map(|e| Ok(e.decode::<RemediationEntry>()?.body.record))
            .collect()
    }
}

impl AuditSink for RemediationJournal {
    fn journal(&mut self, record: &AuditRecord) -> Result<()> {
        let mut log = self
            .log
            .lock()
            .map_err(|_| Error::io("the remediation journal's lock was poisoned"))?;
        let next = log.len() as u64 + 1;
        let at = self.clock.now();
        let event = Envelope::new(
            qip_core::EventId::from_string(format!("evt-aiops-{next}")),
            at,
            at,
            qip_core::lineage::Lineage::root(
                qip_core::CorrelationId::from_string(record.incident.clone()),
                REMEDIATION_ORIGIN,
            ),
            RemediationEntry {
                record: record.clone(),
            },
        )
        .erase()?;
        log.append(&event)?;
        Ok(())
    }
}

/// The effector: this node's switchable capabilities, and nothing else.
#[derive(Debug, Default)]
pub struct CapabilitySwitch {
    disabled: BTreeSet<String>,
}

impl CapabilitySwitch {
    pub fn is_disabled(&self, capability: &str) -> bool {
        self.disabled.contains(capability)
    }
}

impl Effector for CapabilitySwitch {
    fn apply(&mut self, action: &str, target: &str) -> Result<String> {
        if action != "disable_capability" {
            return Err(Error::denied(format!(
                "this node's effector carries out disable_capability only, not {action}"
            )));
        }
        if !CAPABILITIES.contains(&target) {
            return Err(Error::denied(format!(
                "`{target}` is not a capability this node can switch off; it can switch off {CAPABILITIES:?}"
            )));
        }
        if self.disabled.insert(target.to_string()) {
            Ok(format!("{target} disabled until the process restarts"))
        } else {
            Ok(format!("{target} was already disabled"))
        }
    }
}

/// What the operations loop observed of one iteration.
#[derive(Debug, Clone)]
pub struct IterationFacts<'a> {
    pub at: Timestamp,
    pub cycle: u64,
    pub cycle_started_at: Timestamp,
    /// Wall time of the cycle plus everything run in the gap after it.
    pub iteration_ms: f64,
    pub problems: usize,
    /// Anomalies the research sweep found, where one ran.
    pub anomalies: Option<usize>,
    /// Models past their drift threshold, by reference.
    pub drifted: &'a [String],
    /// Whether the learning round promoted a model package this iteration.
    pub package_promoted: bool,
}

/// An incident: a level shift, its timeline and what remediation did.
#[derive(Debug, Clone)]
pub struct Incident {
    pub id: String,
    pub shift: Anomaly,
    pub timeline: Vec<TimelineEvent>,
    /// The remediation's result, or the refusal's reason.
    pub remediation: std::result::Result<String, String>,
    /// Proposed actions no runbook names, left for a person (ADR 0213).
    pub for_a_person: Vec<HumanAction>,
    /// The drafted replay fixture, or why none could be drafted.
    pub fixture: std::result::Result<FixtureLink, String>,
    /// The incident record's journal sequence and its outbox draft's link,
    /// or why the record could not be written. The action, if one ran, ran
    /// either way and its audit records are on the journal already.
    pub recorded: std::result::Result<(u64, FixtureLink), String>,
    /// Severity and owning entity (R12-OBS-1172).
    pub triage: Triage,
}

/// What one re-run of a drafted fixture found (R12-OBS-1172, ADR 0460).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureOutcome {
    /// It reproduces, and the level after it is back at the pre-shift
    /// baseline.
    Cleared,
    /// It reproduces, and the level after it is still shifted.
    StillShifted,
    /// It reproduces; fewer than one detector half has been recorded since.
    AwaitingSamples,
    /// Its window no longer replays to its shift, or cannot be replayed.
    NoLongerReproduces,
}

impl FixtureOutcome {
    /// The `outcome` label value. A fixed set of literals.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cleared => "cleared",
            Self::StillShifted => "still_shifted",
            Self::AwaitingSamples => "awaiting_samples",
            Self::NoLongerReproduces => "no_longer_reproduces",
        }
    }
}

/// One fixture's check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixtureCheck {
    pub incident: String,
    pub outcome: FixtureOutcome,
}

/// Whether `signal` on `entity` is abnormal for triage: a model reported
/// drifted, or a venue fill past [`VENUE_BEHAVIOUR_BPS`] — the same two
/// tests that put either on an incident's timeline, so triage and timeline
/// cannot disagree about what was unusual.
fn is_abnormal(entity: &EntityKey, signal: &Signal) -> bool {
    match entity.kind.as_str() {
        "model" => signal.name == "drifted",
        "venue" => signal.name == "fill_error_bps" && signal.value.abs() > VENUE_BEHAVIOUR_BPS,
        _ => false,
    }
}

/// Check one fixture against `book`'s samples (see
/// [`Operations::check_fixtures`]). A journal that cannot be read is an
/// error, not a verdict: "the test could not run" is not "the test failed".
pub fn check_fixture(book: &RemediationJournal, fixture: &ReplayFixture) -> Result<FixtureOutcome> {
    let samples = book.samples()?;
    match replay_fixture(book, fixture) {
        Ok(Some(found)) if found == fixture.expected => {}
        _ => return Ok(FixtureOutcome::NoLongerReproduces),
    }
    let (from, to) = fixture.journal_sequences;
    let baseline: Vec<(Timestamp, f64)> = samples
        .iter()
        .filter(|(seq, _, s)| (from..=to).contains(seq) && s.at >= fixture.expected.window_start)
        .take(fixture.half)
        .map(|(_, _, s)| (s.at, s.iteration_ms))
        .collect();
    let after: Vec<(Timestamp, f64)> = samples
        .iter()
        .filter(|(seq, _, _)| *seq > to)
        .map(|(_, _, s)| (s.at, s.iteration_ms))
        .collect();
    if baseline.len() < fixture.half || after.len() < fixture.half {
        return Ok(FixtureOutcome::AwaitingSamples);
    }
    let mut points = baseline;
    points.extend_from_slice(&after[after.len() - fixture.half..]);
    Ok(
        match detect_level_shift(&fixture.series, &points, fixture.half)? {
            None => FixtureOutcome::Cleared,
            Some(_) => FixtureOutcome::StillShifted,
        },
    )
}

/// What the loop did this iteration.
#[derive(Debug, Clone, Default)]
pub struct OperationsRound {
    pub incident: Option<Incident>,
    /// `succeeded` or `failed` where the cycle attempted a quantum solve.
    pub quantum: Option<&'static str>,
    /// Critical-path objectives measured and missed this iteration.
    pub objectives_missed: Vec<String>,
    /// What the autonomous test loop found, on an iteration it ran.
    pub fixture_checks: Option<Vec<FixtureCheck>>,
}

impl OperationsRound {
    pub fn describe(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(incident) = &self.incident {
            parts.push(format!(
                "incident {} [{}, owned by {} {}]: {} ({} timeline event(s)); remediation: {}",
                incident.id,
                incident.triage.severity.as_str(),
                incident.triage.owner.kind,
                incident.triage.owner.id,
                TimelineEvent::from(&incident.shift).detail,
                incident.timeline.len(),
                match &incident.remediation {
                    Ok(done) => done.clone(),
                    Err(why) => format!("refused: {why}"),
                }
            ));
            if !incident.for_a_person.is_empty() {
                parts.push(format!(
                    "for a person: {}",
                    incident
                        .for_a_person
                        .iter()
                        .map(|a| format!("{} {}", a.action, a.target))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            parts.push(match &incident.fixture {
                Ok(link) => format!("replay fixture drafted ({})", link.digest),
                Err(why) => format!("no replay fixture: {why}"),
            });
            parts.push(match &incident.recorded {
                Ok((sequence, _)) => {
                    format!("incident journalled at {sequence} and drafted, nothing sent")
                }
                Err(why) => format!("incident record not written: {why}"),
            });
        }
        if let Some(outcome) = self.quantum {
            parts.push(format!("quantum job {outcome}"));
        }
        if !self.objectives_missed.is_empty() {
            parts.push(format!(
                "objectives missed: {}",
                self.objectives_missed.join(", ")
            ));
        }
        // Only the checks a person must act on: a fixture that no longer
        // reproduces, or a shift that has not cleared.
        for check in self.fixture_checks.iter().flatten() {
            if matches!(
                check.outcome,
                FixtureOutcome::NoLongerReproduces | FixtureOutcome::StillShifted
            ) {
                parts.push(format!(
                    "fixture for {} {}",
                    check.incident,
                    check.outcome.as_str()
                ));
            }
        }
        (!parts.is_empty()).then(|| format!("operations: {}", parts.join("; ")))
    }
}

/// The quantum outcome a cycle's journalled routing records, if a job
/// resolved in it.
///
/// Since the quantum arm went submit-and-poll, the cycle that attempts QAOA
/// is not the cycle that learns how it went: a submission or a job still out
/// is not an outcome, and counting it as a failure — what reading the
/// "attempting QAOA" note did — would page operations for every job in
/// flight. So the outcome is read from the typed dispatch record: a harvested
/// job whose answer was judged and ran succeeded; a harvested job dropped as
/// stale or refused by the judge, an abandoned one and a lost one failed.
pub fn quantum_outcome(
    routing: &qip_kernel::platform::SolverRoutingJournal,
) -> Option<&'static str> {
    let dispatch = &routing.quantum_dispatch;
    if dispatch.harvested.is_some() {
        return Some(if routing.ran.iter().any(|s| s == QUANTUM_SOLVER) {
            "succeeded"
        } else {
            "failed"
        });
    }
    (dispatch.abandoned.is_some() || dispatch.lost.is_some()).then_some("failed")
}

fn in_memory_book() -> Result<RemediationJournal> {
    RemediationJournal::open_pair(None, None, Arc::new(qip_core::SystemClock))
}

#[derive(Debug)]
pub struct Operations {
    deployment: EntityKey,
    graph: EntityGraph,
    /// Each retained iteration with the journal sequence of its sample.
    iterations: VecDeque<(Timestamp, f64, u64)>,
    timeline: VecDeque<TimelineEvent>,
    remediator: Remediator<CapabilitySwitch>,
    /// Where samples and incident records are journalled and drafts are
    /// written. In memory unless the node built this with
    /// [`Self::journalled`].
    book: RemediationJournal,
    incidents: u64,
    /// The end of the last shift an incident was opened on, so one shift is
    /// one incident however many iterations still see it in the window.
    last_shift_end: Option<Timestamp>,
    fills_read: usize,
    /// When the last model package was promoted (R12-OBS-9151).
    package_promoted_at: Option<Timestamp>,
    /// Iterations observed by this process, for the test loop's cadence.
    observed: u64,
}

impl Operations {
    /// Samples, incident records and drafts go to an in-memory journal and
    /// outbox; the remediator keeps its in-process trail only.
    pub fn new(started: Timestamp) -> Result<Self> {
        Self::build(started, None, in_memory_book()?)
    }

    /// As [`Self::new`], with every remediation's audit record written to
    /// `journal` before the effector runs (R12-CICD-9081), and samples,
    /// incident records and drafts written to `journal` and its outbox
    /// (ADR 0213). A deployed node is built this way: an in-process trail
    /// does not survive the restart a disabled capability is waiting for.
    pub fn journalled(started: Timestamp, journal: RemediationJournal) -> Result<Self> {
        Self::build(started, Some(Box::new(journal.clone())), journal)
    }

    /// As [`Self::journalled`], to any sink: for a test that must make the
    /// journal fail at a chosen record. The book is in memory.
    pub fn journalled_to(started: Timestamp, sink: Box<dyn AuditSink>) -> Result<Self> {
        Self::build(started, Some(sink), in_memory_book()?)
    }

    /// The journal and outbox this loop writes samples, incident records and
    /// drafts to.
    pub const fn book(&self) -> &RemediationJournal {
        &self.book
    }

    fn build(
        started: Timestamp,
        journal: Option<Box<dyn AuditSink>>,
        book: RemediationJournal,
    ) -> Result<Self> {
        // A restarted node numbers on from the incidents its journal holds,
        // so an incident id names one incident and one fixture.
        let incidents = book.incidents()?.len() as u64;
        let mut timeline = VecDeque::new();
        timeline.push_back(TimelineEvent {
            source: TimelineSource::Deployment,
            at: started,
            detail: "qip-deepbrain process started".to_string(),
        });
        Ok(Self {
            deployment: EntityKey::new("deployment", "qip-deepbrain"),
            graph: EntityGraph::with_retention(GRAPH_RETENTION)?,
            iterations: VecDeque::new(),
            timeline,
            remediator: match journal {
                Some(sink) => {
                    Remediator::journalled(runbooks(), CapabilitySwitch::default(), sink)?
                }
                None => Remediator::new(runbooks(), CapabilitySwitch::default())?,
            },
            book,
            incidents,
            last_shift_end: None,
            fills_read: 0,
            package_promoted_at: None,
            observed: 0,
        })
    }

    /// Whether a runbook has switched `capability` off.
    pub fn is_disabled(&self, capability: &str) -> bool {
        self.remediator.effector().is_disabled(capability)
    }

    pub const fn graph(&self) -> &EntityGraph {
        &self.graph
    }

    pub fn remediator(&self) -> &Remediator<CapabilitySwitch> {
        &self.remediator
    }

    fn push_event(&mut self, event: TimelineEvent) {
        if self.timeline.len() == TIMELINE_RETAINED {
            self.timeline.pop_front();
        }
        self.timeline.push_back(event);
    }

    fn signal(
        &mut self,
        class: SignalClass,
        name: &str,
        value: f64,
        at: Timestamp,
        extra: &[EntityKey],
    ) -> Result<()> {
        let mut entities = vec![self.deployment.clone()];
        entities.extend_from_slice(extra);
        self.graph.record(
            &Signal {
                class,
                name: name.to_string(),
                value,
                at,
            },
            &entities,
        )
    }

    /// Attach scored fills — `(venue, error in bps, scored at)` — to the
    /// graph as trading behaviour, on the venue and the deployment, and put
    /// a fill past [`VENUE_BEHAVIOUR_BPS`] on the timeline.
    pub fn observe_fills(
        &mut self,
        fills: &[(String, f64, Timestamp)],
        at: Timestamp,
    ) -> Result<()> {
        for (venue, bps, scored_at) in fills {
            let key = EntityKey::new("venue", venue.as_str());
            self.signal(
                SignalClass::TradingBehaviour,
                "fill_error_bps",
                *bps,
                at,
                std::slice::from_ref(&key),
            )?;
            if bps.abs() > VENUE_BEHAVIOUR_BPS {
                self.push_event(TimelineEvent {
                    source: TimelineSource::VenueBehaviour,
                    at: *scored_at,
                    detail: format!("{venue} filled {bps:.1} bps away from the twin"),
                });
            }
        }
        Ok(())
    }

    /// Observe one iteration: feed the graph, look for a shift, open and
    /// remediate an incident, record the quantum outcome and the objectives.
    pub fn observe(
        &mut self,
        platform: &Platform,
        facts: &IterationFacts<'_>,
    ) -> Result<OperationsRound> {
        let metrics = platform.telemetry().metrics.clone();
        let mut round = OperationsRound::default();
        let at = facts.at;

        self.signal(
            SignalClass::Infrastructure,
            "iteration_ms",
            facts.iteration_ms,
            at,
            &[],
        )?;
        self.signal(
            SignalClass::Application,
            "stage_problems",
            facts.problems as f64,
            at,
            &[],
        )?;
        if let Some(anomalies) = facts.anomalies {
            self.signal(
                SignalClass::DataQuality,
                "anomalies",
                anomalies as f64,
                at,
                &[],
            )?;
            if anomalies > 0 {
                self.push_event(TimelineEvent {
                    source: TimelineSource::DataAnomaly,
                    at,
                    detail: format!("{anomalies} price anomaly(ies) in cycle {}", facts.cycle),
                });
            }
        }
        for reference in facts.drifted {
            let model = EntityKey::new("model", reference.as_str());
            self.signal(
                SignalClass::ModelHealth,
                "drifted",
                1.0,
                at,
                std::slice::from_ref(&model),
            )?;
            self.push_event(TimelineEvent {
                source: TimelineSource::ModelDrift,
                at,
                detail: format!("{reference} drifted past its threshold"),
            });
        }
        let fills = platform.fill_scores();
        if fills.len() < self.fills_read {
            self.fills_read = 0;
        }
        let fresh: Vec<(String, f64, Timestamp)> = fills[self.fills_read..]
            .iter()
            .filter_map(|f| {
                f.trade_error_bps
                    .map(|bps| (f.venue.clone(), bps, f.scored_at))
            })
            .collect();
        self.fills_read = fills.len();
        self.observe_fills(&fresh, at)?;
        metrics.gauge(
            names::AIOPS_ENTITIES,
            labels([]),
            self.graph.node_count() as f64,
        );
        // Model freshness, recorded every iteration so it grows between
        // promotions: a gauge written only at a promotion would read zero
        // for ever after it, the stalest model looking the freshest.
        if facts.package_promoted {
            self.package_promoted_at = Some(at);
        }
        if let Some(promoted) = self.package_promoted_at {
            metrics.gauge(
                names::MODEL_PACKAGE_AGE,
                labels([]),
                at.since(promoted).as_secs_f64(),
            );
            // The age read against the objective's ceiling, so the gauge's
            // staleness is a good/total split an objective can be evaluated
            // on (R12-OBS-9164).
            let ceiling = qip_observability::slo::MODEL_PACKAGE_MAX_AGE_SECONDS;
            let outcome = if at.since(promoted).as_secs_f64() <= ceiling {
                "fresh"
            } else {
                "stale"
            };
            metrics.count(
                names::MODEL_PACKAGE_FRESHNESS_CHECKS,
                labels([("outcome", outcome)]),
            );
        }

        // The sample is journalled before the detector may read it, so every
        // point a shift is found in is a record a fixture can name. One the
        // journal refuses never enters the series: the series and the journal
        // must hold the same points or a fixture would not replay.
        if !facts.iteration_ms.is_finite() {
            return Err(Error::numeric(
                "a non-finite iteration time reached the operations loop; it is neither \
                 journalled nor read by the detector",
            ));
        }
        let entry = self.cycle_entry(platform, facts)?;
        let sample = IterationSample {
            at,
            cycle: facts.cycle,
            iteration_ms: facts.iteration_ms,
            platform_sequence: platform.event_log().records().last().map(|r| r.sequence),
            correlation_id: entry.as_ref().map(|e| e.correlation_id.clone()),
        };
        let sequence = self.book.journal_entry(
            SAMPLE_ORIGIN,
            &format!("deepbrain-cycle-{}", facts.cycle),
            at,
            sample,
        )?;
        if self.iterations.len() == SERIES_RETAINED {
            self.iterations.pop_front();
        }
        self.iterations
            .push_back((at, facts.iteration_ms, sequence));

        // The node's paths are evaluated by the node, against the registry
        // that records their series; here they would read unmeasured forever.
        // Before the detector, so an incident opened this iteration is
        // triaged against the objectives this iteration measured.
        let statuses = critical_paths::evaluate_where(&metrics.snapshot(), |path| {
            path.slo.service != critical_paths::EDGE_NODE_SERVICE
        });
        critical_paths::record(&metrics, &statuses);
        round.objectives_missed = statuses
            .iter()
            .filter(|s| s.is_observed() && !s.is_met)
            .map(|s| s.slo.name.clone())
            .collect();

        if self.iterations.len() >= 2 * SHIFT_HALF {
            let points: Vec<(Timestamp, f64)> =
                self.iterations.iter().map(|(t, v, _)| (*t, *v)).collect();
            if let Some(shift) = detect_level_shift(ITERATION_SERIES, &points, SHIFT_HALF)?
                && self
                    .last_shift_end
                    .is_none_or(|end| shift.window_start > end)
            {
                round.incident =
                    Some(self.open_incident(shift, at, &metrics, &round.objectives_missed)?);
            }
        }

        if let Some(routing) = entry.and_then(|e| e.solver_routing)
            && let Some(outcome) = quantum_outcome(&routing)
        {
            metrics.count(names::QUANTUM_JOBS, labels([("outcome", outcome)]));
            round.quantum = Some(outcome);
        }

        // The autonomous test loop: every drafted fixture re-run against the
        // journal once per detector half, the fewest fresh samples that can
        // change a verdict (R12-OBS-1172, ADR 0460).
        self.observed += 1;
        if self.observed.is_multiple_of(SHIFT_HALF as u64) {
            let checks = self.check_fixtures()?;
            for check in &checks {
                metrics.count(
                    names::AIOPS_FIXTURE_CHECKS,
                    labels([("outcome", check.outcome.as_str())]),
                );
            }
            round.fixture_checks = Some(checks);
        }
        Ok(round)
    }

    /// The actions the diagnosis proposes for an iteration-time shift, by
    /// comparing the shift with what changed: always shedding this node's
    /// optional load, and scaling the node; and a rollback of the model
    /// package where one was promoted inside the incident's lookback. Only
    /// the first is a runbook's; the others are a person's.
    fn proposals(&self, shift: &Anomaly) -> Vec<(&'static str, &'static str)> {
        let mut proposed = vec![
            ("disable_capability", "research"),
            ("scale", "qip-deepbrain"),
        ];
        if self.package_promoted_at.is_some_and(|promoted| {
            promoted >= shift.window_start.saturating_sub(INCIDENT_LOOKBACK)
        }) {
            proposed.push(("rollback", "model-package"));
        }
        proposed
    }

    /// The fixture for `shift`: the retained series as a journal window,
    /// replayed from the journal before it is drafted. A window that does
    /// not replay to `shift` is refused, and with it every automated action.
    fn draft_fixture(&self, id: &str, shift: &Anomaly, at: Timestamp) -> Result<FixtureLink> {
        let (Some(first), Some(last)) = (self.iterations.front(), self.iterations.back()) else {
            return Err(Error::invalid(
                "an incident with no samples has no window to replay",
            ));
        };
        let (from, to) = (first.2, last.2);
        let window: Vec<(u64, String, IterationSample)> = self
            .book
            .samples()?
            .into_iter()
            .filter(|(sequence, _, _)| (from..=to).contains(sequence))
            .collect();
        let platform: Vec<u64> = window
            .iter()
            .filter_map(|(_, _, s)| s.platform_sequence)
            .collect();
        let fixture = ReplayFixture {
            incident: id.to_string(),
            series: ITERATION_SERIES.to_string(),
            half: SHIFT_HALF,
            journal_sequences: (from, to),
            samples: window.len(),
            window_digest: window_digest(window.iter().map(|(_, hash, _)| hash.as_str())),
            platform_sequences: platform
                .iter()
                .min()
                .copied()
                .zip(platform.iter().max().copied()),
            correlation_ids: window
                .iter()
                .filter_map(|(_, _, s)| s.correlation_id.clone())
                .collect(),
            expected: shift.clone(),
            proposed_test: format!(
                "replay journal sequences {from}..={to} of the operations journal through \
                 detect_level_shift({ITERATION_SERIES:?}, half {SHIFT_HALF}) and assert it \
                 finds this shift; commit it before the fix, then show a window recorded after \
                 the fix replaying to no shift"
            ),
        };
        if replay_fixture(&self.book, &fixture)?.as_ref() != Some(shift) {
            return Err(Error::invalid(
                "the journalled window does not replay to the shift the detector found",
            ));
        }
        self.book.draft(id, at, OperationsDraft::Fixture(fixture))
    }

    fn open_incident(
        &mut self,
        shift: Anomaly,
        at: Timestamp,
        metrics: &Metrics,
        objectives_missed: &[String],
    ) -> Result<Incident> {
        self.incidents += 1;
        let id = format!("INC-deepbrain-{:06}", self.incidents);
        self.last_shift_end = Some(shift.window_end);
        self.push_event(TimelineEvent::from(&shift));
        metrics.count(names::AIOPS_INCIDENTS, labels([]));
        let events: Vec<TimelineEvent> = self.timeline.iter().cloned().collect();
        // From an hour before the shift began, so what led into it is on the
        // same page as the shift itself.
        let timeline = build_timeline(
            shift.window_start.saturating_sub(INCIDENT_LOOKBACK),
            at,
            &events,
        )?;

        // The regression becomes a test before anything is done about it.
        let fixture = self
            .draft_fixture(&id, &shift, at)
            .map_err(|e| e.message().to_string());

        // Only a proposal a committed runbook names reaches the remediator;
        // every other one is a person's, written into the record with why.
        let (automated, others): (Vec<_>, Vec<_>) = self
            .proposals(&shift)
            .into_iter()
            .partition(|(action, target)| runbook_for(action, target).is_some());
        let for_a_person: Vec<HumanAction> = others
            .into_iter()
            .map(|(action, target)| HumanAction {
                action: action.to_string(),
                target: target.to_string(),
                why: FOR_A_PERSON.to_string(),
            })
            .collect();

        let mut results = Vec::new();
        let mut refusal = None;
        for (action, target) in automated {
            if let Err(why) = &fixture {
                metrics.count(
                    names::AIOPS_REMEDIATIONS,
                    labels([("action", action), ("outcome", "withheld_no_fixture")]),
                );
                refusal.get_or_insert_with(|| {
                    format!("withheld: no replay fixture could be drafted ({why})")
                });
                continue;
            }
            match self.remediator.remediate_outcome(&id, action, target) {
                Ok(applied) => {
                    // An action that ran is counted as run even when the sink
                    // then refused its outcome record: the capability is off
                    // either way, and `refused` would tell an operator it is on.
                    let (outcome, note) = match &applied.unjournalled {
                        None => ("applied", String::new()),
                        Some(why) => (
                            "applied_unjournalled",
                            format!("; its outcome could not be journalled: {why}"),
                        ),
                    };
                    metrics.count(
                        names::AIOPS_REMEDIATIONS,
                        labels([("action", action), ("outcome", outcome)]),
                    );
                    results.push(format!("{}{note}", applied.result));
                }
                Err(refused) => {
                    metrics.count(
                        names::AIOPS_REMEDIATIONS,
                        labels([("action", action), ("outcome", "refused")]),
                    );
                    refusal.get_or_insert_with(|| refused.message().to_string());
                }
            }
        }
        let remediation = match refusal {
            Some(why) => Err(why),
            None if results.is_empty() => Err("no runbook names any proposed action".to_string()),
            None => Ok(results.join("; ")),
        };

        // Triage over the same window the timeline covers: which entity owns
        // the incident, and how much it matters (R12-OBS-1172).
        let implicated = self.graph.implicated(
            shift.window_start.saturating_sub(INCIDENT_LOOKBACK),
            at,
            &self.deployment,
            is_abnormal,
        );
        let triage = triage(
            &self.deployment,
            implicated,
            remediation.is_ok(),
            objectives_missed,
        );
        metrics.count(
            names::AIOPS_INCIDENTS_TRIAGED,
            labels([("severity", triage.severity.as_str())]),
        );

        // Every audit record the remediator wrote for this incident, read
        // back from its trail rather than restated, so the record and the
        // trail cannot disagree about what was done.
        let actions: Vec<AuditRecord> = self
            .remediator
            .audit()
            .records()
            .iter()
            .filter(|r| r.incident == id)
            .cloned()
            .collect();
        let record = IncidentRecord {
            incident: id.clone(),
            opened_at: at,
            shift: shift.clone(),
            timeline: timeline.clone(),
            actions,
            for_a_person: for_a_person.clone(),
            fixture: fixture.clone(),
            channel: OUTBOX_CHANNEL.to_string(),
            triage: Some(triage.clone()),
        };
        let recorded = self
            .book
            .journal_entry(INCIDENT_ORIGIN, &id, at, record.clone())
            .and_then(|sequence| {
                self.book
                    .draft(&id, at, OperationsDraft::Incident(record))
                    .map(|link| (sequence, link))
            })
            .map_err(|e| e.message().to_string());
        Ok(Incident {
            id,
            shift,
            timeline,
            remediation,
            for_a_person,
            fixture,
            recorded,
            triage,
        })
    }

    /// The autonomous test loop's body: every fixture in the outbox, re-run.
    ///
    /// Two checks per fixture. First, its window is replayed from the journal
    /// and must still find the shift it was drafted on — a regression test
    /// that no longer reproduces its regression guards nothing, whether the
    /// journal moved under it or the detector changed. Second, the shift's
    /// own pre-shift half is set beside the newest half of samples recorded
    /// after the window, and the detector is asked whether the level is
    /// still shifted from that baseline: the "after the fix" half of the
    /// fixture's own `proposed_test`, run by the process instead of waiting
    /// for a person to.
    pub fn check_fixtures(&self) -> Result<Vec<FixtureCheck>> {
        let mut checks = Vec::new();
        for draft in self.book.drafts()? {
            let OperationsDraft::Fixture(fixture) = draft.draft else {
                continue;
            };
            let outcome = check_fixture(&self.book, &fixture)?;
            checks.push(FixtureCheck {
                incident: fixture.incident.clone(),
                outcome,
            });
        }
        Ok(checks)
    }

    /// The cycle's journal entry, read back from the log rather than from a
    /// field the kernel would have to expose for this.
    fn cycle_entry(
        &self,
        platform: &Platform,
        facts: &IterationFacts<'_>,
    ) -> Result<Option<qip_kernel::CycleJournalEntry>> {
        use qip_events::EventBody;
        let filter = qip_events::EventFilter::new()
            .topic(qip_kernel::CycleJournalEntry::TOPIC)
            .between(facts.cycle_started_at, Timestamp::MAX);
        for envelope in platform.replay_journal(&filter)? {
            let Ok(entry) = envelope.decode::<qip_kernel::CycleJournalEntry>() else {
                continue;
            };
            if entry.body.cycle == facts.cycle {
                return Ok(Some(entry.body));
            }
        }
        Ok(None)
    }
}
