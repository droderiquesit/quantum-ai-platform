//! The contract every connector is run through before it is believed.
//!
//! A connector is a small piece of code that decides what the platform thinks
//! the world is doing. The mistakes are the same every time — a manifest that
//! polls faster than its own rate limit, a decoder that takes the event time
//! from the wrong field, a cursor that resumes from another source's
//! checkpoint, a payload that loses a field and produces records anyway. So
//! they are checked once, here, and every connector is run through the same
//! checks against recorded fixtures.
//!
//! # Why a report rather than assertions
//!
//! The harness returns a [`ContractReport`] instead of panicking, so a test
//! can assert on the whole report and a failure names *every* check that
//! failed rather than the first. A connector that fails four checks and is
//! fixed one at a time is four rounds of the test suite; a report is one.
//!
//! # What the harness does not prove
//!
//! It proves a connector is self-consistent against a recording. It cannot
//! prove the recording still matches the source — only a live fetch does that,
//! and that is what the opt-in live tests in `tests/live_connectors.rs` are
//! for. A connector that passes here and fails there has a stale fixture,
//! which is a finding rather than a contradiction.

use super::SourceConnector;
use super::checkpoint::{Checkpoint, Cursor};
use super::emulator::{RecordedAnswer, RecordedExchange, SourceEmulator};
use super::manifest::{Protocol, SourceManifest};
use super::runtime::{ConnectorRuntime, PollOutcome, RuntimeConfig};
use super::transport::SourceTransport;
use qip_core::Timestamp;
use qip_core::error::Result;
use serde::{Deserialize, Serialize};

/// One check, named as the sentence it asserts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractCheck {
    pub name: String,
    pub passed: bool,
    /// What was actually observed. Present on a pass as well as a failure, so
    /// a report is readable as a description of the connector rather than only
    /// as a list of complaints.
    pub detail: String,
}

impl ContractCheck {
    fn passed(name: &str, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            passed: true,
            detail: detail.into(),
        }
    }

    fn failed(name: &str, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            passed: false,
            detail: detail.into(),
        }
    }
}

/// Every check, and what each one saw.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractReport {
    pub source_id: String,
    pub checks: Vec<ContractCheck>,
}

impl ContractReport {
    pub fn passed(&self) -> bool {
        self.checks.iter().all(|check| check.passed)
    }

    pub fn failures(&self) -> Vec<&ContractCheck> {
        self.checks.iter().filter(|check| !check.passed).collect()
    }

    pub fn check(&self, name: &str) -> Option<&ContractCheck> {
        self.checks.iter().find(|check| check.name == name)
    }

    /// The whole report as text, for a test failure message that has to say
    /// which contract a connector broke.
    pub fn describe(&self) -> String {
        let mut lines = vec![format!("contract report for `{}`:", self.source_id)];
        for check in &self.checks {
            let mark = if check.passed { "ok  " } else { "FAIL" };
            lines.push(format!("  {mark} {} — {}", check.name, check.detail));
        }
        lines.join("\n")
    }
}

/// Runs a connector through the contract.
#[derive(Debug)]
pub struct ContractHarness {
    at: Timestamp,
    seed: u64,
}

impl ContractHarness {
    /// `at` is the instant every check runs at. It must be at or after the
    /// newest event in the fixtures, or every event would be withheld as not
    /// yet knowable and the harness would report a connector that decodes
    /// nothing — which is a harness problem wearing a connector's clothes.
    pub const fn new(at: Timestamp) -> Self {
        Self {
            at,
            seed: 0x00c0_ffee_0bad_f00d,
        }
    }

    pub const fn seeded(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    fn config(&self) -> RuntimeConfig {
        RuntimeConfig::seeded(self.seed)
            .with_dedup_capacity(1_024)
            .with_quarantine_capacity(64)
    }

    /// Run every check.
    ///
    /// The emulator is rewound first, so a harness run is independent of
    /// whatever a test did with the fixtures before it.
    pub fn run(
        &self,
        connector: &mut dyn SourceConnector,
        emulator: &mut SourceEmulator,
    ) -> Result<ContractReport> {
        emulator.rewind();
        let manifest = connector.manifest().clone();
        let mut checks = vec![
            self.manifest_validates(&manifest),
            self.credentials_are_by_reference(&manifest),
            self.poll_interval_respects_the_rate_limit(&manifest),
            self.contract_names_a_required_field(&manifest),
        ];
        checks.extend(self.lifecycle(connector, emulator, &manifest)?);
        checks.push(self.a_foreign_checkpoint_is_refused(connector, &manifest));
        checks.push(self.a_broken_payload_is_quarantined(connector, &manifest)?);
        Ok(ContractReport {
            source_id: manifest.source_id.clone(),
            checks,
        })
    }

    fn manifest_validates(&self, manifest: &SourceManifest) -> ContractCheck {
        const NAME: &str = "the manifest validates";
        match manifest.validate() {
            Ok(()) => ContractCheck::passed(
                NAME,
                format!(
                    "`{}` from {} over {}",
                    manifest.source_id,
                    manifest.provider,
                    manifest.protocol.as_str()
                ),
            ),
            Err(error) => ContractCheck::failed(NAME, error.message()),
        }
    }

    fn credentials_are_by_reference(&self, manifest: &SourceManifest) -> ContractCheck {
        const NAME: &str = "the manifest names a secret variable rather than carrying a credential";
        match &manifest.auth.secret {
            None => ContractCheck::passed(NAME, "the source needs no credential"),
            Some(secret) => match secret.validate() {
                Ok(()) => ContractCheck::passed(
                    NAME,
                    format!("the credential is read from `{}`", secret.variable()),
                ),
                Err(error) => ContractCheck::failed(NAME, error.message()),
            },
        }
    }

    fn poll_interval_respects_the_rate_limit(&self, manifest: &SourceManifest) -> ContractCheck {
        const NAME: &str = "the poll interval stays inside the source's own rate limit";
        let floor = manifest.rate_limit.min_interval();
        if manifest.poll_interval() >= floor {
            ContractCheck::passed(
                NAME,
                format!(
                    "polls every {:?}; the limit permits one request per {floor:?}",
                    manifest.poll_interval()
                ),
            )
        } else {
            ContractCheck::failed(
                NAME,
                format!(
                    "polls every {:?} and the limit permits one request per {floor:?}",
                    manifest.poll_interval()
                ),
            )
        }
    }

    fn contract_names_a_required_field(&self, manifest: &SourceManifest) -> ContractCheck {
        const NAME: &str = "the schema contract names at least one required field";
        if manifest.schema.required_fields.is_empty() {
            ContractCheck::failed(
                NAME,
                "no required fields, so a source that stopped sending everything this connector \
                 reads would pass the schema gate and produce nothing, with no error",
            )
        } else {
            ContractCheck::passed(
                NAME,
                format!(
                    "{} required field(s) at schema {}",
                    manifest.schema.required_fields.len(),
                    manifest.schema.version
                ),
            )
        }
    }

    /// connect → health → fetch → dedup → checkpoint → resume → shutdown, in
    /// that order, against the recorded fixtures.
    fn lifecycle(
        &self,
        connector: &mut dyn SourceConnector,
        emulator: &mut SourceEmulator,
        manifest: &SourceManifest,
    ) -> Result<Vec<ContractCheck>> {
        let mut checks = Vec::new();
        // A manifest that does not validate cannot build a runtime, and that is
        // a finding rather than a harness failure: the report has to say which
        // check failed, not stop before the checks run.
        let mut runtime = match ConnectorRuntime::new(manifest.clone(), self.config()) {
            Ok(runtime) => runtime,
            Err(error) => {
                checks.push(ContractCheck::failed(
                    "the manifest builds a runtime",
                    error.message(),
                ));
                return Ok(checks);
            }
        };
        let transport: &mut dyn SourceTransport = emulator;

        match runtime.connect(connector, transport, self.at) {
            Ok(health) => checks.push(ContractCheck::passed(
                "the connector connects and answers a health check",
                health.detail,
            )),
            Err(error) => {
                checks.push(ContractCheck::failed(
                    "the connector connects and answers a health check",
                    error.message(),
                ));
                return Ok(checks);
            }
        }

        let first = runtime.poll(connector, transport, self.at)?;
        if first.outcome == PollOutcome::Delivered && !first.admitted.is_empty() {
            checks.push(ContractCheck::passed(
                "a first fetch decodes at least one event into the canonical envelope",
                format!(
                    "{} admitted, {} withheld, {} quarantined",
                    first.admitted.len(),
                    first.withheld,
                    first.quarantined
                ),
            ));
        } else {
            checks.push(ContractCheck::failed(
                "a first fetch decodes at least one event into the canonical envelope",
                format!(
                    "outcome {:?}, {} admitted, {} withheld, {} quarantined. The fixture's newest \
                     event may be later than the harness instant, in which case every event is \
                     correctly withheld and the harness is pointed at the wrong instant",
                    first.outcome,
                    first.admitted.len(),
                    first.withheld,
                    first.quarantined
                ),
            ));
        }

        checks.push(self.envelopes_keep_the_instants_apart(&first, manifest));

        // The same page again. A healthy source re-serving its last answer is
        // the redelivery every overlapping poll window produces, and nothing
        // new must come out of it.
        let second = runtime.poll(connector, transport, self.at)?;
        if second.admitted.is_empty() && second.duplicates >= first.admitted.len() as u64 {
            checks.push(ContractCheck::passed(
                "re-serving the same page produces duplicates and no new events",
                format!("{} duplicate(s) absorbed", second.duplicates),
            ));
        } else {
            checks.push(ContractCheck::failed(
                "re-serving the same page produces duplicates and no new events",
                format!(
                    "{} event(s) admitted a second time and {} recognised as duplicates: the \
                     fingerprint is not stable across two decodes of the same bytes",
                    second.admitted.len(),
                    second.duplicates
                ),
            ));
        }

        checks.push(self.checkpoint_round_trips(&runtime, connector));

        match runtime.shutdown(connector, self.at) {
            Ok(()) => checks.push(ContractCheck::passed(
                "the connector shuts down without error",
                format!(
                    "{} protocol; {}",
                    manifest.protocol.as_str(),
                    if manifest.protocol.requires_buffering() {
                        "a pushed feed, so shutdown must drain what it buffered"
                    } else {
                        "a pulled feed, so shutdown releases the connection"
                    }
                ),
            )),
            Err(error) => checks.push(ContractCheck::failed(
                "the connector shuts down without error",
                error.message(),
            )),
        }
        Ok(checks)
    }

    fn envelopes_keep_the_instants_apart(
        &self,
        report: &super::runtime::PollReport,
        manifest: &SourceManifest,
    ) -> ContractCheck {
        const NAME: &str = "every envelope's event time comes from the payload and is not the \
                            ingest time";
        let delay = manifest.publication_delay();
        for envelope in &report.admitted {
            if envelope.event_time() > envelope.ingest_time() {
                return ContractCheck::failed(
                    NAME,
                    format!(
                        "`{}` has an event time of {} and an ingest time of {}, which is earlier",
                        envelope.upstream_key(),
                        envelope.event_time(),
                        envelope.ingest_time()
                    ),
                );
            }
            if envelope.knowable_at() != envelope.event_time().saturating_add(delay) {
                return ContractCheck::failed(
                    NAME,
                    format!(
                        "`{}` is knowable at {} and its event time plus the manifest's {delay:?} \
                         dissemination delay is {}",
                        envelope.upstream_key(),
                        envelope.knowable_at(),
                        envelope.event_time().saturating_add(delay)
                    ),
                );
            }
        }
        ContractCheck::passed(
            NAME,
            format!(
                "{} envelope(s) carry an event time from the payload, an ingest time from the \
                 caller's horizon, and a knowable time {delay:?} after the event",
                report.admitted.len()
            ),
        )
    }

    fn checkpoint_round_trips(
        &self,
        runtime: &ConnectorRuntime,
        connector: &mut dyn SourceConnector,
    ) -> ContractCheck {
        const NAME: &str = "a checkpoint round-trips through JSON and resumes to the same cursor";
        let taken = runtime.checkpoint(self.at);
        let restored = match taken
            .to_json()
            .and_then(|text| Checkpoint::from_json(&text))
        {
            Ok(restored) => restored,
            Err(error) => return ContractCheck::failed(NAME, error.message()),
        };
        match connector.resume(&restored) {
            Ok(cursor) if cursor == taken.cursor => ContractCheck::passed(
                NAME,
                format!(
                    "resumed at {:?} after {} event(s)",
                    cursor.position, cursor.events_seen
                ),
            ),
            Ok(cursor) => ContractCheck::failed(
                NAME,
                format!(
                    "resumed at {:?} and the checkpoint held {:?}",
                    cursor.position, taken.cursor.position
                ),
            ),
            Err(error) => ContractCheck::failed(NAME, error.message()),
        }
    }

    fn a_foreign_checkpoint_is_refused(
        &self,
        connector: &mut dyn SourceConnector,
        manifest: &SourceManifest,
    ) -> ContractCheck {
        const NAME: &str = "a checkpoint written by another source is refused rather than resumed";
        let mut foreign = manifest.clone();
        foreign.source_id = format!("{}-impostor", manifest.source_id);
        let checkpoint = Checkpoint::new(&foreign, Cursor::at_event_time(self.at), self.at);
        match connector.resume(&checkpoint) {
            Ok(cursor) => ContractCheck::failed(
                NAME,
                format!(
                    "resumed at {:?} from a checkpoint belonging to `{}`: a gap on one side and a \
                     replay on the other, both silent",
                    cursor.position, foreign.source_id
                ),
            ),
            Err(error) => ContractCheck::passed(NAME, error.message()),
        }
    }

    /// A payload with none of the contract's required fields must reach the
    /// quarantine, not the decoder.
    fn a_broken_payload_is_quarantined(
        &self,
        connector: &mut dyn SourceConnector,
        manifest: &SourceManifest,
    ) -> Result<ContractCheck> {
        const NAME: &str = "a payload missing the contract's required fields is quarantined \
                            rather than decoded";
        let mut runtime = match ConnectorRuntime::new(manifest.clone(), self.config()) {
            Ok(runtime) => runtime,
            Err(error) => return Ok(ContractCheck::failed(NAME, error.message())),
        };
        let request = connector.fetch_request(&Cursor::beginning())?;
        let mut broken = SourceEmulator::new(vec![RecordedExchange::always(
            request.path.clone(),
            RecordedAnswer::json(
                200,
                r#"{"contract_harness":"a well-formed body with none of the required fields"}"#,
            ),
        )]);
        let report = runtime.poll(connector, &mut broken, self.at)?;
        if report.quarantined > 0 && report.admitted.is_empty() {
            let reasons: Vec<String> = runtime
                .quarantine()
                .recent(1)
                .iter()
                .map(|entry| entry.reason.code().to_string())
                .collect();
            Ok(ContractCheck::passed(
                NAME,
                format!("quarantined as {}", reasons.join(", ")),
            ))
        } else {
            Ok(ContractCheck::failed(
                NAME,
                format!(
                    "{} admitted and {} quarantined from a payload with none of the required \
                     fields",
                    report.admitted.len(),
                    report.quarantined
                ),
            ))
        }
    }
}

/// The protocols the harness knows how to drive.
///
/// A `websocket` connector buffers rather than fetching inside the call, so
/// the fetch checks above describe it only after its buffer has been fed. This
/// is stated as a function rather than left implicit so that adding a pushed
/// source is a compile-time decision about the harness rather than a run that
/// quietly proves nothing.
pub const fn harness_drives(protocol: Protocol) -> bool {
    matches!(protocol, Protocol::Rest | Protocol::Poll | Protocol::File)
}
