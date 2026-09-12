//! Where records come from.
//!
//! Both sources this build has are [`DataAdapter`]s, which is why this module
//! is thin: the run loop pulls with a timestamp it owns, so the synthetic
//! exchange, a recorded replay and a licensed feed that does not exist yet are
//! the same seam rather than three shapes of loop.
//!
//! Nothing here is production-grade, and the node says so at start-up rather
//! than letting a synthetic tape look like a tape. `is_production_grade` comes
//! off the adapter's own descriptor, so a source that started claiming to be
//! licensed would change this answer without anything here being edited.
//!
//! Every record is validated before the platform sees it. A record that fails
//! is counted and reported rather than dropped: bad data must never silently
//! become an investment input, and a rejection nobody counts is a silent one.
//!
//! # Two gates before a connector opens
//!
//! A catalogued connector passes the data finder's licensing evaluation and
//! then its registration registry, in that order, before anything is
//! constructed. The registry is the one the node's own configuration stands
//! for — [`qip_kernel::PlatformConfig::registration_registry`] — so a source
//! whose venue demands an account opens here only when the owner recorded the
//! registration they made, and is refused **by name** when they did not. The
//! node used to ask [`qip_data_finder::admission::admit`], which consults the
//! shipped table and holds nobody's record: that door could never admit an
//! account-gated source however the deployment was configured, and it was the
//! wrong answer for the right reason.
//!
//! The licensing half does not stop at start-up. A connector arm holds a
//! [`qip_data_finder::admission::StandingAdmission`] and re-asks the whole gate
//! at the instant of every [`Feed::poll`], before the socket: a node asked to
//! stream for a week outlives the instant its licence was evaluated at, and one
//! that expired on day three used to keep granting for the remaining four. The
//! arm also keeps a durable stream record when the composition root gives it a
//! store through [`Feed::journal_to`], so a restart resumes the dedup window
//! the last process left instead of republishing the source's whole table.
//!
//! Both gates are table reads. **Nothing here consults a language model, and
//! nothing may** — ADR 0008 puts no model on the fast path at all, and a
//! licence and a registration are facts somebody wrote down and reviewed, not
//! judgements to be inferred at start-up. A model asked whether a source may
//! be traded on would answer, plausibly, without having read anything.

use crate::config::{ConnectorFeedSettings, LiveFeedSettings};
use qip_core::error::{Error, Result};
use qip_core::kv::KeyValueStore;
use qip_core::{Clock, Duration, ManualClock, ObjectId, Timestamp};
use qip_data_finder::admission::{self, CatalogueEntry, StandingAdmission};
use qip_data_finder::registration::RegistrationRegistry;
use qip_financial::quality::LicensingClass;
use qip_market::bar::Interval;
use qip_market_ingestion::adapter::{DataAdapter, SensedRecord, SourceDescriptor};
use qip_market_ingestion::connector_feed::ConnectorFeed;
use qip_market_ingestion::replay::ReplayAdapter;
use qip_market_ingestion::rest::{RestFeedConfig, RestInstrument, RestMarketDataAdapter};
use qip_market_ingestion::synthetic::{EnvironmentConfig, SyntheticEnvironment};
use qip_market_ingestion::tape::{Tape, TapeFeed};
use std::sync::Arc;

/// One poll's worth of records, split by whether they may be believed.
#[derive(Debug, Default)]
pub struct Batch {
    /// Records that passed validation, ready for the platform.
    pub accepted: Vec<SensedRecord>,
    /// Why each rejected record was rejected.
    pub rejections: Vec<String>,
    /// The content hash of exactly what a connector fetched, for the kernel's
    /// reference ledger. `None` for every other arm and for a connector poll
    /// that delivered nothing to digest. The node hands it to
    /// `Platform::reference_fetch` *before* `Platform::observe`.
    pub digest: Option<qip_market_ingestion::connector::FetchDigest>,
}

impl Batch {
    pub fn is_empty(&self) -> bool {
        self.accepted.is_empty() && self.rejections.is_empty()
    }
}

/// The node's record source.
///
/// An enum rather than a `Box<dyn DataAdapter>` because the loop needs one
/// question the trait does not answer: whether a replay has run out. A replay
/// that has reached its last record is a finished session, and a node that kept
/// cycling on an empty feed would look busy and be idle.
#[derive(Debug)]
pub enum Feed {
    /// The in-tree synthetic exchange. Boxed because the environment carries a
    /// whole market and the enum is moved around the loop.
    Synthetic(Box<SyntheticEnvironment>),
    /// Recorded records from a JSONL file.
    Replay(Box<ReplayAdapter>),
    /// A committed bitemporal tape, run one period per cycle on the tape's
    /// own clock. The difference from `Replay` is the clock: a replay on the
    /// wall clock is swallowed in one poll, so nothing that takes tape time
    /// — a horizon resolving, a claim being scored — ever happens.
    Tape(Box<TapeFeed>),
    /// A licensed vendor, polled over the in-cluster egress proxy.
    ///
    /// No environment in this repository configures one. The variant exists so
    /// that adding a vendor is configuration rather than engineering — see
    /// `LiveFeedSettings` for why the licence, the host and the credential are
    /// decisions this code deliberately does not make.
    Live(Box<RestMarketDataAdapter>),
    /// A worked connector from the ingestion SDK, opened through the egress
    /// proxy after the licensing catalogue admitted it and the registration
    /// registry named who registered. See
    /// [`qip_data_finder::admission::admit_registered`] — both gates run
    /// before construction, and the only constructors that reach this arm are
    /// [`Self::connector`] and [`Self::connector_admitted_by`], each of which
    /// runs them.
    ///
    /// The gate is *kept* rather than discarded once it has answered. A node
    /// asked to stream for a week outlives the instant its licence was
    /// evaluated at, and a licence that expires on day three of a seven-day
    /// run used to keep granting for the remaining four because nothing asked
    /// again. [`Self::poll`] re-asks the whole of it at the instant of every
    /// poll, which is what makes this a control rather than the shape of one.
    Connector {
        feed: Box<ConnectorFeed>,
        admission: Box<StandingAdmission>,
        /// The source as the kernel's reference ledger sees it — the gate's
        /// decision bound to the manifest's declared category (ADR 0057).
        /// The composition root hands it to `Platform::admit_source` so every
        /// digest this arm produces can be referenced.
        admitted: Box<qip_data_finder::admission::AdmittedSource>,
    },
}

impl Feed {
    /// Open the synthetic exchange, seeded so a session is reproducible.
    ///
    /// The bar cadence is chosen from the step rather than left at the
    /// environment's minute default. The platform's SENSE stage reads price
    /// history, and price history is fed by bars: a node cycling ten times a
    /// second against minute bars spends the first six hundred cycles
    /// reporting that it is running blind, and would be right.
    pub fn synthetic(seed: u64, step: Duration, start: Timestamp) -> Self {
        let config = EnvironmentConfig {
            seed,
            step,
            bar_interval: bar_interval_for(step),
            ..EnvironmentConfig::default()
        };
        Self::Synthetic(Box::new(SyntheticEnvironment::demo(start, config)))
    }

    /// Open a recorded feed, refusing a file with nothing in it.
    ///
    /// An empty replay is almost always a wrong path rather than an empty
    /// session, and a node that started and immediately reported "the feed is
    /// exhausted" would have hidden the mistake behind a clean exit.
    pub fn replay(path: &str) -> Result<Self> {
        let adapter = ReplayAdapter::open("replay", path)?;
        if adapter.is_empty() {
            return Err(Error::invalid(format!(
                "the replay file {path} holds no record this node can read; check the path before \
                 checking the node"
            )));
        }
        Ok(Self::Replay(Box::new(adapter)))
    }

    /// Open a committed tape on its own clock.
    ///
    /// Every refusal — leakage, disorder, an incoherent bar, an empty file —
    /// is the tape loader's and is not restated here.
    pub fn tape(path: &str) -> Result<Self> {
        Ok(Self::Tape(Box::new(TapeFeed::new(Tape::open(path)?))))
    }

    /// Open a licensed vendor through the egress proxy.
    ///
    /// The licensing class is `Licensed` and not a configurable: this
    /// constructor is reached only when an operator has stated a vendor, a
    /// path, a venue and a credential, and a feed configured that far is a
    /// licensed one. Making it settable would let a deployment label a live
    /// tape `Synthetic` — which reads as caution and is the opposite, because
    /// `LicensingClass::Synthetic` is the one class barred from production
    /// decisions, so the label would quietly stop real prices from being
    /// acted on while the node reported a live feed.
    pub fn live(settings: &LiveFeedSettings, venue: &str) -> Result<Self> {
        let instruments: Vec<RestInstrument> = settings
            .symbols
            .iter()
            .map(|symbol| {
                RestInstrument::new(
                    ObjectId::from_string(symbol.as_str()),
                    symbol.clone(),
                    venue,
                )
            })
            .collect();
        let config = RestFeedConfig {
            base_url: Some(settings.base_url.clone()),
            path: settings.path.clone(),
            api_key: Some(settings.api_key.clone()),
            api_key_header: settings.api_key_header.clone(),
            licensing: LicensingClass::Licensed,
            ..RestFeedConfig::default()
        };
        Ok(Self::Live(Box::new(RestMarketDataAdapter::new(
            config,
            instruments,
        )?)))
    }

    /// Open a catalogued connector source through the egress proxy.
    ///
    /// Both gates run here, before anything is constructed and before any
    /// socket is touched: the rule is evaluation *then* use, and putting the
    /// call inside the constructor makes the ordering a property of the code
    /// path rather than of the caller's memory. The catalogue is the data
    /// finder's, shared with `qip-api`: this root once carried its own copy
    /// of the same entries, and two catalogues that must say the same thing
    /// about one licence are one catalogue plus a drift nobody has found yet.
    ///
    /// `registrations` is the registry the node's configuration stands for,
    /// so a source is admitted here only on a record the platform — assembled
    /// afterwards from the same configuration — also holds. Passed in rather
    /// than reached for: a constructor that built its own registry would
    /// answer from a table nothing else in the process shares.
    ///
    /// Both questions are answered by reading a table. Nothing on this path
    /// asks a model anything (ADR 0008); see the module documentation for why
    /// that is a rule and not an omission.
    pub fn connector(
        settings: &ConnectorFeedSettings,
        registrations: &RegistrationRegistry,
        at: Timestamp,
    ) -> Result<Self> {
        let class = qip_market_ingestion::connector_feed::shipped_class(&settings.source_id)?;
        let (admission, decision) =
            StandingAdmission::open(registrations.clone(), &settings.source_id, class, at)?;
        Self::admitted_connector(settings, admission, &decision, at)
    }

    /// The same opening against a caller-supplied licensing catalogue.
    ///
    /// Split from [`Self::connector`] so a test can hold the registration
    /// question against an entry the real catalogue must never contain —
    /// today every account-gated source is refused on its unread terms first,
    /// so without this the registration gate could only be proven by the
    /// refusal it does not produce.
    pub fn connector_admitted_by(
        entries: &[CatalogueEntry],
        registrations: &RegistrationRegistry,
        settings: &ConnectorFeedSettings,
        at: Timestamp,
    ) -> Result<Self> {
        let class = qip_market_ingestion::connector_feed::shipped_class(&settings.source_id)?;
        let (admission, decision) = StandingAdmission::over(
            entries.to_vec(),
            registrations.clone(),
            &settings.source_id,
            class,
            at,
        )?;
        Self::admitted_connector(settings, admission, &decision, at)
    }

    /// Construct the connector arm, once something has admitted it.
    ///
    /// Private, and the two callers above are the whole set: a public
    /// constructor here would be a door into the arm that skips both gates,
    /// which is the thing the ordering exists to prevent.
    fn admitted_connector(
        settings: &ConnectorFeedSettings,
        admission: StandingAdmission,
        decision: &qip_data_finder::admission::LicensingDecision,
        at: Timestamp,
    ) -> Result<Self> {
        let feed = ConnectorFeed::open(&settings.source_id, &settings.base_url, settings.seed, at)?;
        // Refused before the arm exists for a manifest that declares no
        // category: a connector the node can poll but the platform cannot
        // reference would fetch bytes nothing could later be asked about.
        let admitted =
            qip_data_finder::admission::AdmittedSource::from_decision(decision, feed.manifest())?;
        Ok(Self::Connector {
            feed: Box::new(feed),
            admission: Box::new(admission),
            admitted: Box::new(admitted),
        })
    }

    /// The admission the kernel's reference ledger needs for this source, or
    /// `None` for every arm that is not a licensed connector — a tape, a
    /// replay and the synthetic exchange are this repository's own records.
    pub fn admitted_source(&self) -> Option<&qip_data_finder::admission::AdmittedSource> {
        match self {
            Self::Connector { admitted, .. } => Some(admitted.as_ref()),
            Self::Synthetic(_) | Self::Replay(_) | Self::Tape(_) | Self::Live(_) => None,
        }
    }

    /// Keep this source's stream record on `store`, resuming the last
    /// process's position, and say how many fingerprints came back.
    ///
    /// `None` for every arm but a connector: a tape, a replay and the
    /// synthetic exchange carry their own records and have no vendor to
    /// redeliver from, so there is nothing for a dedup window to absorb.
    ///
    /// The failure this closes at the root: without it every restart began
    /// with an empty dedup window, so the whole of a table-shaped source —
    /// the ECB reference rates are one table per poll — was republished as
    /// new observations at every rollout, and the session and span figures an
    /// operator would need to claim a source had streamed for a week reset to
    /// zero at each start-up. Call it before the first
    /// [`Self::poll`]: the dedup window refuses a restore once it has observed
    /// anything, so a later call is a refusal rather than a partial restore.
    pub fn journal_to(&mut self, store: Arc<dyn KeyValueStore>) -> Result<Option<usize>> {
        match self {
            Self::Connector { feed, .. } => feed.journal_to(store).map(Some),
            Self::Synthetic(_) | Self::Replay(_) | Self::Tape(_) | Self::Live(_) => Ok(None),
        }
    }

    /// Choose a source from the configuration.
    ///
    /// Ordered by how much a wrong choice costs. A configured vendor wins
    /// outright: an operator who supplied a licence and a credential and got
    /// the synthetic exchange anyway would be the worst outcome available
    /// here, because nothing downstream can tell the two tapes apart once the
    /// records look the same.
    ///
    /// `registrations` reaches only the connector arm, and it is a parameter
    /// rather than a default so the one registry the process holds is the one
    /// the gate reads. Every other arm needs no account: a tape, a replay and
    /// the synthetic exchange are this repository's own files.
    pub fn open(
        live: Option<&LiveFeedSettings>,
        connector: Option<&ConnectorFeedSettings>,
        replay_path: Option<&str>,
        tape_path: Option<&str>,
        registrations: &RegistrationRegistry,
        seed: u64,
        step: Duration,
        start: Timestamp,
    ) -> Result<Self> {
        match (live, connector, replay_path, tape_path) {
            // Two live sources at once is not a precedence question, it is a
            // configuration contradiction: whichever this code preferred, the
            // operator meant the other one somewhere. Refused outright.
            (Some(_), Some(_), _, _) => Err(Error::invalid(
                "both a live vendor (QIP_MARKET_DATA_*) and a connector source \
                 (QIP_CONNECTOR_*) are configured. One node reads one live source; \
                 unset one of them",
            )),
            (Some(settings), None, _, _) => {
                let venue = settings.venue.clone();
                Self::live(settings, &venue)
            }
            (None, Some(settings), _, _) => Self::connector(settings, registrations, start),
            // The same contradiction one rung down: two recordings, and the
            // two run on different clocks, so there is no answer that is
            // right for both.
            (None, None, Some(_), Some(_)) => Err(Error::invalid(
                "both QIP_FASTBRAIN_REPLAY_PATH and QIP_FASTBRAIN_TAPE_PATH are set. A replay \
                 runs on the wall clock and a tape on its own; unset one of them",
            )),
            (None, None, Some(path), None) => Self::replay(path),
            (None, None, None, Some(path)) => Self::tape(path),
            (None, None, None, None) => Ok(Self::synthetic(seed, step, start)),
        }
    }

    fn adapter_mut(&mut self) -> &mut dyn DataAdapter {
        match self {
            Self::Synthetic(environment) => environment.as_mut(),
            Self::Replay(adapter) => adapter.as_mut(),
            Self::Tape(adapter) => adapter.as_mut(),
            Self::Live(adapter) => adapter.as_mut(),
            Self::Connector { feed, .. } => feed.as_mut(),
        }
    }

    pub fn descriptor(&self) -> SourceDescriptor {
        match self {
            Self::Synthetic(environment) => environment.descriptor(),
            Self::Replay(adapter) => adapter.descriptor(),
            Self::Tape(adapter) => adapter.descriptor(),
            Self::Live(adapter) => adapter.descriptor(),
            Self::Connector { feed, .. } => feed.descriptor(),
        }
    }

    /// The clock this source owns, if it owns one.
    ///
    /// A tape does; everything else runs on the wall clock. The platform's
    /// `Context` must be built on the clock returned here, or the platform
    /// prices every opportunity as of today while observing last year — and
    /// the cost router, asked for a latency budget that ended months ago,
    /// declines to convene anything.
    pub fn owned_clock(&self) -> Option<Arc<ManualClock>> {
        match self {
            Self::Tape(adapter) => Some(adapter.clock()),
            Self::Synthetic(_) | Self::Replay(_) | Self::Live(_) | Self::Connector { .. } => None,
        }
    }

    /// The instant the next cycle runs at.
    ///
    /// The wall clock for every source but a tape, which is moved one period
    /// forward and read. `None` only for a spent tape, which the loop has
    /// already stopped on through [`Self::is_exhausted`].
    pub fn cycle_instant(&mut self, wall: &dyn Clock) -> Option<Timestamp> {
        match self {
            Self::Tape(adapter) => adapter.advance(),
            Self::Synthetic(_) | Self::Replay(_) | Self::Live(_) | Self::Connector { .. } => {
                Some(wall.now())
            }
        }
    }

    /// The current instant on whichever clock this source runs on.
    pub fn now(&self, wall: &dyn Clock) -> Timestamp {
        match self.owned_clock() {
            Some(clock) => clock.now(),
            None => wall.now(),
        }
    }

    /// Refuse a tape that outlasts the organisation's authorisation.
    ///
    /// The platform stamps every manifest reviewed at assembly, which on a
    /// tape is the tape's first instant, and refuses to run an agent once
    /// `now` reaches the review interval. A tape longer than the interval
    /// therefore runs its remaining periods with every panel refused — the
    /// 320-day daily tape this was written against convened its first panel
    /// on tape day 103 and reported eighteen agents `failed` on every panel
    /// after, which read as an agent defect and was governance working as
    /// designed. Refused at start-up instead, naming the two spans, because
    /// nothing inside a replay can re-review a roster. A source that owns no
    /// clock has nothing to check.
    pub fn refuse_tape_beyond(&self, review_interval: Duration) -> Result<()> {
        let Self::Tape(adapter) = self else {
            return Ok(());
        };
        let Some((first, last)) = adapter.tape().span() else {
            return Ok(());
        };
        let span = last.since(first);
        if span >= review_interval {
            return Err(Error::invalid(format!(
                "the tape spans {:.1} day(s), from {} to {}, and the organisation's authorisation \
                 lapses {:.1} day(s) after assembly; every panel after that would be refused. \
                 Shorten the tape or use a finer interval; a roster cannot be re-reviewed \
                 inside a replay",
                span.as_days_f64(),
                first.to_rfc3339(),
                last.to_rfc3339(),
                review_interval.as_days_f64()
            )));
        }
        Ok(())
    }

    /// One line on the tape, for the banner: what is on it and when.
    pub fn tape_summary(&self) -> Option<String> {
        let Self::Tape(adapter) = self else {
            return None;
        };
        let tape = adapter.tape();
        let (first, last) = tape.span()?;
        Some(format!(
            "{} observation(s) across {} instrument(s) in {} period(s), {} to {}; tape time \
             drives the platform clock, one period per cycle",
            tape.len(),
            tape.instruments().len(),
            tape.periods(),
            first.to_rfc3339(),
            last.to_rfc3339()
        ))
    }

    /// The standing licensing gate's own account of itself, for the banner.
    ///
    /// `None` for a source no licence gates. Printed rather than kept private
    /// because the number that distinguishes a gate consulted on every poll
    /// from one consulted at start-up is its check count, and an operator who
    /// cannot read it can only believe the claim.
    pub fn licensing_standing(&self) -> Option<String> {
        match self {
            Self::Connector {
                admission,
                admitted,
                ..
            } => Some(format!("{}; {}", admission.describe(), admitted.describe())),
            Self::Synthetic(_) | Self::Replay(_) | Self::Tape(_) | Self::Live(_) => None,
        }
    }

    /// Whether records from this source may drive a real capital decision.
    pub fn is_production_grade(&self) -> bool {
        self.descriptor().is_production_grade()
    }

    /// What a production deployment would still have to supply, if anything.
    pub fn production_requirement(&self) -> Option<String> {
        self.descriptor().production_requirement
    }

    /// Whether this source has nothing left to give.
    ///
    /// Always false for the synthetic exchange: it generates forever, which is
    /// what makes it useful for a node meant to stay up.
    pub fn is_exhausted(&self) -> bool {
        match self {
            Self::Synthetic(_) => false,
            Self::Replay(adapter) => adapter.remaining() == 0,
            Self::Tape(adapter) => adapter.remaining() == 0,
            // A vendor stops answering; it does not run out.
            Self::Live(_) | Self::Connector { .. } => false,
        }
    }

    /// Pull everything available up to `until`, validating as it goes.
    ///
    /// A connector's licensing gate is re-asked at `until` **before** the
    /// socket, and a refusal returns here rather than producing an empty
    /// batch: an empty batch and a source this platform is no longer licensed
    /// to read are indistinguishable downstream, and the second one must stop
    /// the node rather than quietly starve it. The rule is evaluation *then*
    /// use, and it is a rule about every use rather than about the first.
    pub fn poll(&mut self, until: Timestamp) -> Result<Batch> {
        if let Self::Connector { admission, .. } = self {
            admission.check(until)?;
        }
        let source = self.descriptor().name;
        let mut batch = Batch::default();
        for record in self.adapter_mut().poll(until)? {
            let issues = record.validate();
            if issues.is_empty() {
                batch.accepted.push(record);
            } else {
                batch.rejections.push(format!(
                    "{source} produced an unusable {}: {}",
                    record.topic().name(),
                    issues.join("; ")
                ));
            }
        }
        if let Self::Connector { feed, .. } = self {
            batch.digest = feed.take_digest();
        }
        Ok(batch)
    }
}

/// Where every catalogued source stands, one banner line each.
///
/// Read off the registry the process holds, so the line describes the table
/// the gate above reads rather than a second one assembled for display. Every
/// catalogued source is named, not only the admitted ones: a banner listing
/// what opened would read as a node with nothing to register, and the source
/// an operator needs to see is the one that is refused.
///
/// A refusal is reported as its requirement rather than as the registry's
/// whole sentence — the sentence names the runbook, the credential and the
/// venue, and four of them would bury the banner. The full refusal is what a
/// configured-but-unregistered source stops the process with, which is where
/// an operator meets it.
///
/// This is a table read and nothing else. No model is asked what a source's
/// standing is, here or anywhere on this node (ADR 0008).
pub fn source_standings(registrations: &RegistrationRegistry) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    for entry in admission::catalogue()? {
        let standing = match registrations.standing(entry.source_id) {
            Ok(standing) => standing.describe(),
            Err(_) => match registrations.requirement(entry.source_id) {
                Some(requirement) => format!(
                    "not registered, so refused: it needs {}",
                    requirement.describe()
                ),
                None => "refused: no registration requirement is declared for it".to_string(),
            },
        };
        lines.push(format!(
            "  registration:     {}: {standing}",
            entry.source_id
        ));
    }
    Ok(lines)
}

/// The finest bar a step of this size can close.
///
/// A bar closes when the step crosses a bucket boundary, so a bucket shorter
/// than the step would still only produce one bar per step while claiming a
/// resolution the data does not have.
fn bar_interval_for(step: Duration) -> Interval {
    if step < Duration::from_mins(1) {
        Interval::Second
    } else {
        Interval::Minute
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    #[test]
    fn the_synthetic_exchange_produces_records_and_never_runs_out() {
        let mut feed = Feed::synthetic(7, Duration::from_secs(60), start());
        let batch = feed
            .poll(start().saturating_add(Duration::from_mins(30)))
            .expect("the synthetic exchange polls");
        assert!(
            !batch.accepted.is_empty(),
            "half an hour of synthetic market produced no record"
        );
        assert!(!feed.is_exhausted());
    }

    #[test]
    fn the_same_seed_over_the_same_span_produces_the_same_records() {
        // Reproducibility is the reason the seed is configuration rather than
        // drawn at start-up: a session that cannot be re-run cannot be debugged.
        let until = start().saturating_add(Duration::from_mins(20));
        let mut first = Feed::synthetic(99, Duration::from_secs(60), start());
        let mut second = Feed::synthetic(99, Duration::from_secs(60), start());
        let a = first.poll(until).expect("polls");
        let b = second.poll(until).expect("polls");
        assert_eq!(a.accepted.len(), b.accepted.len());
        assert_eq!(
            serde_json::to_string(&a.accepted).expect("records serialise"),
            serde_json::to_string(&b.accepted).expect("records serialise")
        );
    }

    #[test]
    fn a_node_that_cycles_faster_than_a_minute_is_fed_bars_faster_than_a_minute() {
        // The blindness this prevents: SENSE reads price history, price history
        // is fed by bars, and a bar that closes once a minute leaves a node
        // cycling ten times a second with nothing to sense for six hundred
        // cycles.
        assert_eq!(
            bar_interval_for(Duration::from_millis(100)),
            Interval::Second
        );
        assert_eq!(bar_interval_for(Duration::from_secs(59)), Interval::Second);
        assert_eq!(bar_interval_for(Duration::from_mins(1)), Interval::Minute);

        let mut feed = Feed::synthetic(31, Duration::from_millis(100), start());
        let batch = feed
            .poll(start().saturating_add(Duration::from_secs(5)))
            .expect("polls");
        let bars = batch
            .accepted
            .iter()
            .filter(|record| matches!(record, qip_market_ingestion::adapter::SensedRecord::Bar(_)))
            .count();
        assert!(
            bars > 0,
            "five seconds of a hundred-millisecond feed closed no bar, so the node would sense \
             nothing"
        );
    }

    #[test]
    fn no_source_this_build_has_is_production_grade_and_each_says_what_is_missing() {
        let feed = Feed::synthetic(1, Duration::from_secs(60), start());
        assert!(
            !feed.is_production_grade(),
            "a synthetic tape must never pass for a licensed one"
        );
        assert!(
            feed.production_requirement().is_some(),
            "the synthetic source does not say what production would have to supply"
        );
    }

    #[test]
    fn a_replay_reports_itself_exhausted_once_its_last_record_has_been_read() {
        let mut source = Feed::synthetic(3, Duration::from_secs(60), start());
        let recorded = source
            .poll(start().saturating_add(Duration::from_mins(10)))
            .expect("polls")
            .accepted;
        assert!(!recorded.is_empty(), "nothing was recorded to replay");

        let directory =
            std::env::temp_dir().join(format!("qip-fastbrain-replay-{}", std::process::id()));
        let path = directory.join("records.jsonl");
        ReplayAdapter::write(&path, &recorded).expect("the replay file is written");

        let mut feed = Feed::replay(&path.display().to_string()).expect("the replay file opens");
        assert!(!feed.is_exhausted(), "a fresh replay has records left");
        let batch = feed.poll(Timestamp::MAX).expect("polls");
        assert_eq!(batch.accepted.len(), recorded.len());
        assert!(
            feed.is_exhausted(),
            "a replay whose records have all been read still claims to have more"
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_replay_file_that_holds_nothing_readable_is_refused_rather_than_opened_empty() {
        let directory =
            std::env::temp_dir().join(format!("qip-fastbrain-empty-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the directory is created");
        let path = directory.join("empty.jsonl");
        std::fs::write(&path, "").expect("the file is written");

        let refusal = Feed::replay(&path.display().to_string())
            .expect_err("an empty replay is a wrong path, not an empty session");
        assert!(
            refusal.message().contains("check the path"),
            "the refusal does not point at the likely cause: {}",
            refusal.message()
        );

        let _ = std::fs::remove_dir_all(&directory);
    }
}

#[cfg(test)]
mod tape_tests {
    use super::*;
    use qip_core::SystemClock;
    use qip_market_ingestion::tape::{SCHEMA_VERSION, TapeDocument, TapeObservation};

    /// A tape of `days` daily bars on one instrument, written to a file.
    fn tape_file(name: &str, days: i64) -> (std::path::PathBuf, std::path::PathBuf) {
        let first = Timestamp::parse_rfc3339("2025-01-06T21:00:00Z").expect("an instant");
        let observations = (0..days)
            .map(|day| {
                let at = first.saturating_add(Duration::from_days(day));
                TapeObservation {
                    object_id: "OBJ-TAPE".to_string(),
                    venue: "XNYS".to_string(),
                    at: at.to_rfc3339(),
                    known_at: at.saturating_add(Duration::from_mins(15)).to_rfc3339(),
                    open: qip_core::Decimal::from_int(100),
                    high: qip_core::Decimal::from_int(101),
                    low: qip_core::Decimal::from_int(99),
                    close: qip_core::Decimal::from_int(100),
                    volume: qip_core::Decimal::from_int(1_000),
                }
            })
            .collect();
        let document = TapeDocument {
            schema_version: SCHEMA_VERSION,
            name: name.to_string(),
            description: "a feed test tape".to_string(),
            interval: Interval::Day,
            observations,
            macro_releases: Vec::new(),
            alternative_data: Vec::new(),
            dividend_declarations: Vec::new(),
        };
        let directory =
            std::env::temp_dir().join(format!("qip-fastbrain-tape-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the directory is created");
        let path = directory.join("tape.json");
        std::fs::write(
            &path,
            serde_json::to_string(&document).expect("the tape serialises"),
        )
        .expect("the tape is written");
        (directory, path)
    }

    #[test]
    fn a_tape_runs_on_its_own_clock_and_not_the_wall_clock() {
        // The property the tape arm exists for. A cycle instant taken from
        // the wall clock would release the whole tape in one poll and no
        // horizon would ever pass on it.
        let (directory, path) = tape_file("clock", 3);
        let mut feed = Feed::tape(&path.display().to_string()).expect("the tape opens");
        let wall = SystemClock;
        let owned = feed.owned_clock().expect("a tape owns a clock");

        let first = feed
            .cycle_instant(&wall)
            .expect("an unread tape has a first period");
        assert_eq!(
            first,
            Timestamp::parse_rfc3339("2025-01-06T21:15:00Z").expect("an instant"),
            "the first cycle is not at the first knowable instant"
        );
        assert_eq!(
            owned.now(),
            first,
            "the owned clock did not follow the tape"
        );
        assert_eq!(feed.now(&wall), first, "`now` did not read the owned clock");
        assert!(
            first < wall.now(),
            "the premise: the tape is in the past, so wall time would have swallowed it"
        );
        assert_eq!(
            feed.poll(first).expect("polls").accepted.len(),
            1,
            "one period released other than one bar"
        );
        assert!(!feed.is_exhausted());

        let second = feed.cycle_instant(&wall).expect("a second period");
        assert_eq!(second.since(first), Duration::from_days(1));
        let _ = feed.poll(second);
        let _ = feed.cycle_instant(&wall);
        let _ = feed.poll(Timestamp::MAX);
        assert!(
            feed.is_exhausted(),
            "a fully read tape still claims records"
        );
        assert!(feed.cycle_instant(&wall).is_none());

        // The other arms answer the wall clock and own nothing.
        let mut synthetic = Feed::synthetic(1, Duration::from_secs(1), first);
        assert!(synthetic.owned_clock().is_none());
        assert!(
            synthetic
                .cycle_instant(&wall)
                .is_some_and(|now| now > first)
        );

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_tape_that_outlasts_the_roster_review_interval_is_refused_at_start_up() {
        // The run that showed why: a 320-day daily tape convened its first
        // panel on tape day 103 and eighteen agents were refused on every
        // panel after, reported as `failed`. Governance working as designed,
        // read as a defect, because nothing said the tape was too long.
        let (directory, long) = tape_file("long", 100);
        let feed = Feed::tape(&long.display().to_string()).expect("the tape opens");
        let refusal = feed
            .refuse_tape_beyond(Duration::from_days(90))
            .expect_err("a 100-day tape was admitted against a 90-day review interval");
        assert!(
            refusal.message().contains("lapses") && refusal.message().contains("Shorten"),
            "the refusal does not say what lapses or what to do: {}",
            refusal.message()
        );
        let _ = std::fs::remove_dir_all(&directory);

        // And the premise on the other side: a tape inside the window is
        // admitted, or the gate refuses everything and proves nothing.
        let (directory, short) = tape_file("short", 10);
        let feed = Feed::tape(&short.display().to_string()).expect("the tape opens");
        feed.refuse_tape_beyond(Duration::from_days(90))
            .expect("a 10-day tape is inside a 90-day review interval");
        // A source with no clock has nothing to check.
        Feed::synthetic(
            1,
            Duration::from_secs(1),
            Timestamp::from_secs(1_760_000_000),
        )
        .refuse_tape_beyond(Duration::ZERO)
        .expect("the synthetic exchange owns no tape to measure");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_replay_and_a_tape_together_are_a_contradiction_and_refused() {
        // A replay runs on the wall clock and a tape on its own. Whichever
        // this code preferred, the operator meant the other one somewhere.
        let (directory, path) = tape_file("both", 2);
        let refusal = Feed::open(
            None,
            None,
            Some(&path.display().to_string()),
            Some(&path.display().to_string()),
            // No arm reached here needs an account; the registry is the
            // shipped table because a tape is this repository's own file.
            &RegistrationRegistry::shipped(),
            7,
            Duration::from_secs(1),
            Timestamp::from_secs(1_760_000_000),
        )
        .expect_err("two recordings on two clocks were opened as one feed");
        assert!(
            refusal.message().contains("QIP_FASTBRAIN_REPLAY_PATH")
                && refusal.message().contains("QIP_FASTBRAIN_TAPE_PATH"),
            "the refusal does not name both variables: {}",
            refusal.message()
        );

        // Alone, the tape opens through the same door.
        let feed = Feed::open(
            None,
            None,
            None,
            Some(&path.display().to_string()),
            &RegistrationRegistry::shipped(),
            7,
            Duration::from_secs(1),
            Timestamp::from_secs(1_760_000_000),
        )
        .expect("a tape alone opens");
        assert!(matches!(feed, Feed::Tape(_)));
        assert!(
            !feed.is_production_grade(),
            "a tape claimed to be admissible for a capital decision"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}

#[cfg(test)]
mod source_choice_tests {
    use super::*;

    fn settings() -> LiveFeedSettings {
        LiveFeedSettings {
            base_url: "http://qip-egress.qip.svc.cluster.local:9105".into(),
            path: "/v1/quotes".into(),
            symbols: vec!["AAPL".into()],
            venue: "XNAS".into(),
            // Short on purpose. A longer placeholder matches
            // `check-secrets.sh`'s credential pattern and makes the scan
            // report a finding on every run, which is how a scanner stops
            // being read.
            api_key: "not-a-key".into(),
            api_key_header: "x-api-key".into(),
        }
    }

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    #[test]
    fn a_configured_vendor_wins_over_a_replay_file() {
        // The failure this ordering exists to prevent. An operator who
        // supplied a licence, a host and a credential, and got a recorded
        // tape instead, would be running the platform on last week's prices
        // while every surface reported a live feed. Nothing downstream can
        // tell the two apart once the records look the same, so the ordering
        // is the only place the distinction can be made.
        let live = settings();
        let feed = Feed::open(
            Some(&live),
            None,
            Some("/tmp/some-recorded-session.jsonl"),
            None,
            &RegistrationRegistry::shipped(),
            7,
            Duration::from_secs(1),
            start(),
        )
        .expect("a configured vendor opens");
        assert!(
            matches!(feed, Feed::Live(_)),
            "a configured vendor was replaced by another source"
        );
    }

    #[test]
    fn a_live_vendor_is_licensed_and_may_drive_a_capital_decision() {
        // `LicensingClass::Synthetic` is the one class barred from production
        // decisions, so labelling a live tape with it would read as caution
        // and do the opposite of what it looks like: real prices would be
        // silently refused as an investment input while the node reported a
        // licensed feed. The class is fixed at this constructor rather than
        // configurable for exactly that reason.
        let live = settings();
        let feed = Feed::live(&live, &live.venue).expect("a vendor opens");
        assert!(
            feed.is_production_grade(),
            "a licensed vendor is not admissible for a capital decision: {:?}",
            feed.descriptor().production_requirement
        );
    }

    #[test]
    fn the_synthetic_exchange_may_not_drive_a_capital_decision() {
        // The premise that makes the assertion above mean something. If every
        // source were production-grade, that test would pass without checking
        // anything — and the synthetic exchange must never be mistaken for a
        // tape, which is the whole reason the descriptor carries the class.
        let feed = Feed::open(
            None,
            None,
            None,
            None,
            &RegistrationRegistry::shipped(),
            7,
            Duration::from_secs(1),
            start(),
        )
        .expect("the synthetic exchange opens");
        assert!(matches!(feed, Feed::Synthetic(_)));
        assert!(
            !feed.is_production_grade(),
            "generated prices claimed to be admissible for a capital decision"
        );
        assert!(
            feed.production_requirement().is_some(),
            "the synthetic exchange does not say what a production deployment \
             would still have to supply"
        );
    }
}

#[cfg(test)]
mod connector_feed_tests {
    use super::*;
    use crate::config::ConnectorFeedSettings;

    fn start() -> Timestamp {
        Timestamp::from_secs(1_760_000_000)
    }

    fn connector_settings() -> ConnectorFeedSettings {
        ConnectorFeedSettings {
            source_id: "coinbase-spot-ticker".to_string(),
            base_url: "http://egress.test:8080".to_string(),
            seed: 7,
        }
    }

    fn live_settings() -> LiveFeedSettings {
        LiveFeedSettings {
            base_url: "http://egress.test:8080".to_string(),
            path: "/v1/market-data".to_string(),
            symbols: vec!["ACME".to_string()],
            venue: "XLON".to_string(),
            api_key: "a-key".to_string(),
            api_key_header: "x-api-key".to_string(),
        }
    }

    #[test]
    fn two_live_sources_at_once_are_a_contradiction_and_not_a_precedence_question() {
        // Whichever source this code preferred, the operator meant the other
        // one somewhere. The refusal is the only answer that cannot be wrong.
        let refused = Feed::open(
            Some(&live_settings()),
            Some(&connector_settings()),
            None,
            None,
            &RegistrationRegistry::shipped(),
            7,
            Duration::from_secs(1),
            start(),
        );
        assert!(
            refused.is_err(),
            "a node configured with two live sources opened one of them anyway"
        );
    }

    #[test]
    fn an_uncatalogued_connector_source_is_refused_before_any_socket_is_touched() {
        // The licensing gate runs inside the constructor, so there is no path
        // to the connector arm around it. An unknown source has no evaluation
        // on file, and the refusal arrives without a network in the test
        // environment — which is itself the evidence that the gate runs
        // before the transport.
        let mut settings = connector_settings();
        settings.source_id = "some-unevaluated-endpoint".to_string();
        let refused = Feed::open(
            None,
            Some(&settings),
            None,
            None,
            &RegistrationRegistry::shipped(),
            7,
            Duration::from_secs(1),
            start(),
        );
        assert!(
            refused.is_err(),
            "an unevaluated source was opened, so its terms were never read"
        );
    }
}
