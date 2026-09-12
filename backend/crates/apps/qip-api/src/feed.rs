//! Where the API's SENSE stage gets its records, when it gets any.
//!
//! `POST /cycle` used to run the loop on a platform nothing observed into:
//! every stage after SENSE reasoned over an empty tape, no claim was ever
//! recorded, and every research route answered — honestly — that nothing had
//! been seen. This module is the composition root's answer to which source
//! the process senses, and there are three:
//!
//! * **None.** The shipped state and the one every deployment is in. The
//!   banner says so, and a cycle reasons over whatever the platform already
//!   holds, which is nothing. This is not a fallback to a synthetic exchange:
//!   a process that generated plausible prices because nobody configured a
//!   source would be indistinguishable downstream from one that sensed a
//!   market, and the API is the process an operator reads.
//! * **The demonstration tape** (`QIP_API_TAPE_PATH`), a committed bitemporal
//!   fixture replayed on its own clock. The platform is assembled on that
//!   clock and each `POST /cycle` moves it one period forward, so a claim
//!   with a five-day horizon resolves five periods later rather than never.
//!   Exactly the tape `qip-fastbrain` runs, through the same
//!   [`TapeFeed`], so the two roots cannot read one file two ways.
//! * **A catalogued connector** (`QIP_CONNECTOR_SOURCE` and
//!   `QIP_CONNECTOR_BASE_URL`), the real path ADR 0034 decides: a worked
//!   connector from the ingestion SDK, opened through the TLS-terminating
//!   egress proxy, after — never before — the data finder's licensing
//!   catalogue has admitted it and its registration registry has named who
//!   registered. [`ApiFeed::connector`] is shaped so there is no route to a
//!   connector around either gate, and the registry the gate consults is
//!   the one the platform is then assembled with
//!   (`PlatformConfig::registration_registry`), so the feed cannot admit a
//!   source on a record the platform does not hold.
//!
//! The connector arm's gate no longer runs only once. [`ApiFeed::readmit`]
//! runs it again on the platform's own registry when an operator approves a
//! registration for the source this process senses, so an approval taken at
//! runtime reaches the feed rather than waiting for a restart nobody was
//! told to make. It re-runs *both* gates against the shipped catalogue, so a
//! registration can never admit a source whose licensing posture still
//! refuses it, and it replaces the open connector only after the new one is
//! built — a refusal leaves the process sensing exactly what it was.
//!
//! Nor does the licensing half run only at start-up. A connector arm holds a
//! [`qip_data_finder::admission::StandingAdmission`] and re-asks the whole gate
//! — catalogue, class agreement, both usages, registration — at the instant of
//! every `POST /cycle`, in [`ApiFeed::sense`], before the socket. A licence that
//! expired on day three of a seven-day run used to keep granting for the
//! remaining four, because the only consultation was at boot.
//!
//! A connector arm also keeps a durable stream record, when the composition
//! root gives it a store through [`ApiFeed::journal_to`]. Without it every
//! restart began with an empty dedup window and republished whatever the source
//! re-serves — the ECB rates are a whole table per poll — as new observations,
//! and the session and span figures reset at every start-up.
//!
//! A tape and a connector at once is a contradiction rather than a
//! precedence question: whichever this code preferred, the operator meant the
//! other one somewhere, and the only answer that cannot be wrong is a refusal
//! that names both variables.

use qip_core::error::{Error, Result};
use qip_core::kv::KeyValueStore;
use qip_core::{Clock, ManualClock, Timestamp};
use qip_data_finder::admission::{
    self, AdmittedSource, CatalogueEntry, LicensingDecision, StandingAdmission,
};
use qip_data_finder::registration::RegistrationRegistry;
use qip_kernel::Platform;
use qip_market_ingestion::adapter::{DataAdapter, SensedRecord, SourceDescriptor};
use qip_market_ingestion::connector::FetchDigest;
use qip_market_ingestion::connector_feed::{ConnectorFeed, shipped_class};
use qip_market_ingestion::tape::{Tape, TapeFeed};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The committed tape this process replays, when it replays one.
pub const TAPE_PATH_VARIABLE: &str = "QIP_API_TAPE_PATH";
/// The catalogued connector source to open, by its manifest's `source_id`.
///
/// The same pair of names `qip-fastbrain` reads, on purpose: a deployment
/// that selected a source once should not discover that the API wanted it
/// spelled differently.
pub const CONNECTOR_SOURCE_VARIABLE: &str = "QIP_CONNECTOR_SOURCE";
/// `http://host[:port]` of the **egress proxy**, never of the vendor.
pub const CONNECTOR_BASE_URL_VARIABLE: &str = "QIP_CONNECTOR_BASE_URL";

/// A catalogued connector source and the egress address to reach it through.
///
/// No credential, because the sources this build carries are unauthenticated
/// by their manifests, and the licensing catalogue is what decides whether a
/// source may be used at all. A future keyed source adds its credential to
/// the manifest's own auth scheme and resolves it through `qip_core::secret`,
/// not here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorSettings {
    pub source_id: String,
    pub base_url: String,
}

/// Which source the deployment chose, resolved from the environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedSettings {
    /// Nothing configured. The shipped state; the banner says so.
    None,
    Tape(String),
    Connector(ConnectorSettings),
}

impl FeedSettings {
    /// Read the process environment.
    pub fn from_env() -> Result<Self> {
        Self::parse(&std::env::vars().collect())
    }

    /// Resolve a selection from a set of variables, refusing a contradictory
    /// or half-configured one.
    ///
    /// Parsing takes a map rather than reading the process environment, so
    /// the refusals are asserted directly instead of by setting variables in
    /// a process that other tests share.
    pub fn parse(vars: &BTreeMap<String, String>) -> Result<Self> {
        let tape = text(vars, TAPE_PATH_VARIABLE);
        let source = text(vars, CONNECTOR_SOURCE_VARIABLE);
        let base_url = text(vars, CONNECTOR_BASE_URL_VARIABLE);

        // Half a connector is refused before the contradiction check so the
        // operator is told about the nearer mistake first; the silent
        // alternative in either case would be a process that starts and
        // senses nothing while its configuration says otherwise.
        let connector = match (source, base_url) {
            (None, None) => None,
            (Some(_), None) => {
                return Err(Error::invalid(format!(
                    "{CONNECTOR_SOURCE_VARIABLE} is set and {CONNECTOR_BASE_URL_VARIABLE} is not. \
                     A connector source needs the egress proxy's address; set both, or neither"
                )));
            }
            (None, Some(_)) => {
                return Err(Error::invalid(format!(
                    "{CONNECTOR_BASE_URL_VARIABLE} is set and {CONNECTOR_SOURCE_VARIABLE} is not. \
                     An egress address with no source names nothing to fetch; set both, or \
                     neither"
                )));
            }
            (Some(source_id), Some(base_url)) => {
                // The transport has no TLS stack, so `https` is refused at
                // construction anyway; saying so here names the deployment
                // mistake instead of surfacing it as a connection error.
                if base_url.starts_with("https://") {
                    return Err(Error::invalid(format!(
                        "{CONNECTOR_BASE_URL_VARIABLE} is {base_url}. `qip_transport::http` speaks \
                         plaintext HTTP/1.1 and has no TLS stack: point this at the egress proxy, \
                         which terminates TLS to the vendor, never at the vendor itself"
                    )));
                }
                Some(ConnectorSettings {
                    source_id,
                    base_url,
                })
            }
        };

        match (tape, connector) {
            (Some(_), Some(_)) => Err(Error::invalid(format!(
                "both {TAPE_PATH_VARIABLE} and {CONNECTOR_SOURCE_VARIABLE} are set. A tape runs on \
                 its own clock and a connector on the wall clock, so there is no cycle instant \
                 that is right for both; unset one of them"
            ))),
            (Some(path), None) => Ok(Self::Tape(path)),
            (None, Some(settings)) => Ok(Self::Connector(settings)),
            (None, None) => Ok(Self::None),
        }
    }
}

/// A non-empty value, trimmed. Empty is treated as unset: a variable set to
/// the empty string in a manifest is a variable somebody forgot to fill in.
fn text(vars: &BTreeMap<String, String>, name: &str) -> Option<String> {
    vars.get(name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Whether this process can actually read the credential the deployment
/// variable `slot` names — without the value ever being bound to a name here.
///
/// `direct` and `path` are the two sources [`qip_core::secret`] resolves
/// between: `slot` itself and `slot_FILE`, the projection the Secret Manager
/// CSI driver writes. They are passed in rather than read here because the
/// caller decides where they come from, and because the process environment
/// cannot be written from a test in this workspace — `std::env::set_var` is
/// unsafe in the 2024 edition and `unsafe` is forbidden at the workspace
/// root, which is the same reason [`FeedSettings::parse`] takes a map.
///
/// The rule itself is not restated: `resolve_from` is the one implementation
/// of the `_FILE` indirection, and a second one here would be a second place
/// for the two sources to disagree. The resolved credential is consumed by
/// `is_some()` on the line it arrives, so no value reaches a log, a response
/// or a stack frame that outlives this call. A refusal names the slot and
/// its `_FILE` variant and nothing else.
///
/// The failure this prevents: re-opening a connector on a registration
/// record whose credential the deployment never mounted. The transport would
/// then fail at the vendor with an authentication error that names neither
/// the variable nor the record, and the operator who had just approved the
/// registration would read it as the venue rejecting them.
pub fn credential_is_readable(
    slot: &str,
    direct: Option<String>,
    path: Option<String>,
) -> Result<()> {
    if qip_core::secret::resolve_from(slot, direct, path)?.is_some() {
        return Ok(());
    }
    Err(Error::invalid(format!(
        "{slot} names no credential this process can read: neither {slot} nor {slot}{suffix} is \
         set here. The registration stands and is in the log; mount the secret under {slot}{suffix} \
         and the connector is admitted at the next approval or the next start",
        suffix = qip_core::secret::FILE_SUFFIX
    )))
}

/// What one `POST /cycle` sensed before the loop ran.
#[derive(Debug)]
pub struct Sensed {
    /// The descriptor's name of the source that answered.
    pub source: String,
    /// The instant the cycle runs at: tape time for a tape, the wall clock
    /// for a connector.
    pub at: Timestamp,
    /// Records that passed validation, ready for the platform.
    pub records: Vec<SensedRecord>,
    /// Why each rejected record was rejected. Counted and reported rather
    /// than dropped: bad data must never silently become an investment
    /// input, and a rejection nobody counts is a silent one.
    pub rejections: Vec<String>,
    /// The content hash of exactly what a connector fetched this cycle, for
    /// the kernel's reference ledger. `None` for a tape, and for a connector
    /// poll that delivered nothing to digest. The route hands it to
    /// `Platform::reference_fetch` *before* `Platform::observe`, so a fetch
    /// the platform cannot reference is one whose records it does not take.
    pub digest: Option<FetchDigest>,
}

/// The process's record source.
///
/// An enum rather than a `Box<dyn DataAdapter>` because the route needs two
/// answers the trait does not give: whether the source owns the clock, and
/// whether a tape has run out. A tape that has reached its last period is a
/// finished demonstration, and a process that kept cycling on it at a frozen
/// instant would look busy and be idle.
#[derive(Debug)]
pub enum ApiFeed {
    /// A committed bitemporal tape on its own clock.
    Tape(Box<TapeFeed>),
    /// A catalogued connector, and the licensing decision that admitted it.
    Connector {
        feed: Box<ConnectorFeed>,
        decision: Box<LicensingDecision>,
        /// What it would take to open this source again: the selection the
        /// composition root resolved and the seed its runtime was built on.
        ///
        /// Kept beside the open connector because a *re*-admission has to be
        /// the same admission. [`ConnectorFeed`] exposes a descriptor and
        /// nothing else, so without this the only way to re-open a source
        /// after a runtime approval would be to read the environment a
        /// second time — and a second read is a second answer the day the
        /// two disagree, which is exactly the class of bug the feed's
        /// single selection at start-up exists to prevent.
        settings: ConnectorSettings,
        seed: u64,
        /// The licensing gate held open for as long as the source is polled.
        ///
        /// `decision` beside it is what the gate said at start-up and is what
        /// the banner reads; this is the gate itself, re-asked at the instant
        /// of every `POST /cycle`. Both, because a decision taken once is a
        /// fact about one instant, and a process that senses for a week is
        /// asked about a great many more: a licence expiring on day three of a
        /// seven-day run kept granting for the remaining four when the only
        /// consultation was at boot.
        admission: Box<StandingAdmission>,
        /// The source as the kernel's reference ledger sees it: the gate's
        /// decision bound to the manifest's declared category and schema
        /// (ADR 0057). Built here, at the one seam that holds both, and
        /// handed to `Platform::admit_source` by the composition root and
        /// again by [`Self::readmit`]'s caller, so every digest this arm
        /// produces can be referenced against an admission the platform
        /// itself holds.
        admitted: Box<AdmittedSource>,
        /// The store this stream's durable record is kept on, when a root has
        /// given one.
        ///
        /// Held rather than passed once so that [`Self::readmit`] can re-attach
        /// it. Re-admission replaces the whole connector, and a journal that
        /// was attached only at start-up would be silently dropped by the first
        /// operator approval — leaving the process streaming with no record and
        /// nothing saying so, which is the exact failure the journal exists to
        /// close, reintroduced through the one door that reopens the source.
        journal: Option<Arc<dyn KeyValueStore>>,
    },
}

impl ApiFeed {
    /// Open whichever source the settings name, or none.
    ///
    /// `registrations` is the registry a connector is admitted against —
    /// the composition root passes the one its configuration stands for, so
    /// a source admitted here is one the platform, assembled afterwards on
    /// the same configuration, also holds a record for. `at` is the instant
    /// a connector is admitted and connected at; a tape starts on its own
    /// first knowable instant and ignores both.
    pub fn open(
        settings: &FeedSettings,
        registrations: &RegistrationRegistry,
        seed: u64,
        at: Timestamp,
    ) -> Result<Option<Self>> {
        match settings {
            FeedSettings::None => Ok(None),
            FeedSettings::Tape(path) => Self::tape(path).map(Some),
            FeedSettings::Connector(connector) => {
                Self::connector_registered(connector, registrations, seed, at).map(Some)
            }
        }
    }

    /// Open a committed tape on its own clock.
    ///
    /// Every refusal — leakage, disorder, an incoherent bar, an empty file —
    /// is the tape loader's and is not restated here.
    pub fn tape(path: &str) -> Result<Self> {
        Ok(Self::Tape(Box::new(TapeFeed::new(Tape::open(path)?))))
    }

    /// Open a catalogued connector through the egress proxy, after the
    /// licensing gate has admitted it, against the shipped registration
    /// registry — which records nobody, so this door opens the keyless
    /// sources and nothing that needs an account. The composition root goes
    /// through [`Self::open`], which carries the owner's records.
    pub fn connector(settings: &ConnectorSettings, seed: u64, at: Timestamp) -> Result<Self> {
        Self::connector_registered(settings, &RegistrationRegistry::shipped(), seed, at)
    }

    /// Open a catalogued connector with the owner's registration records.
    pub fn connector_registered(
        settings: &ConnectorSettings,
        registrations: &RegistrationRegistry,
        seed: u64,
        at: Timestamp,
    ) -> Result<Self> {
        Self::connector_admitted_by_registered(
            &admission::catalogue()?,
            registrations,
            settings,
            seed,
            at,
        )
    }

    /// The same opening against a caller-supplied licensing catalogue and
    /// the shipped registration registry.
    ///
    /// Split from [`Self::connector`] so a test can hold the gate against an
    /// entry the real catalogue must never contain and prove that no socket
    /// opens.
    pub fn connector_admitted_by(
        entries: &[CatalogueEntry],
        settings: &ConnectorSettings,
        seed: u64,
        at: Timestamp,
    ) -> Result<Self> {
        Self::connector_admitted_by_registered(
            entries,
            &RegistrationRegistry::shipped(),
            settings,
            seed,
            at,
        )
    }

    /// The full gate against a caller-supplied catalogue and registry.
    ///
    /// Both gates run here, before anything is constructed and before any
    /// socket is touched: the rule is evaluation *then* use, and putting the
    /// call inside the constructor makes the ordering a property of the code
    /// path rather than of the caller's memory. The licensing question is
    /// asked first and the registration question second, inside
    /// `admit_from_registered`, so a source whose terms are unread is
    /// refused for that and only a source whose terms admit it is asked who
    /// holds its account.
    pub fn connector_admitted_by_registered(
        entries: &[CatalogueEntry],
        registrations: &RegistrationRegistry,
        settings: &ConnectorSettings,
        seed: u64,
        at: Timestamp,
    ) -> Result<Self> {
        let class = shipped_class(&settings.source_id)?;
        // The gate is opened rather than merely consulted: `StandingAdmission`
        // runs the identical admission — the catalogue, the class agreement,
        // both usages and the registration — and returns the same decision, so
        // nothing is weakened here, and what is gained is that the same gate
        // can be re-asked at every poll instead of never again.
        let (admission, decision) = StandingAdmission::over(
            entries.to_vec(),
            registrations.clone(),
            &settings.source_id,
            class,
            at,
        )?;
        let feed = ConnectorFeed::open(&settings.source_id, &settings.base_url, seed, at)?;
        // The reference ledger's view of the source, from the decision the
        // gate just minted and the manifest the feed was opened from. Refused
        // here — before the arm exists — for a manifest that declares no
        // category, because a connector the platform can poll but cannot
        // reference would fetch bytes nothing could later be asked about.
        let admitted = AdmittedSource::from_decision(&decision, feed.manifest())?;
        Ok(Self::Connector {
            feed: Box::new(feed),
            decision: Box::new(decision),
            settings: settings.clone(),
            seed,
            admission: Box::new(admission),
            admitted: Box::new(admitted),
            journal: None,
        })
    }

    /// The admission the kernel's reference ledger needs for this source, or
    /// `None` for a tape, which carries its own records and is referenced
    /// under no vendor.
    pub fn admitted_source(&self) -> Option<&AdmittedSource> {
        match self {
            Self::Tape(_) => None,
            Self::Connector { admitted, .. } => Some(admitted.as_ref()),
        }
    }

    /// Keep this source's stream record on `store`, resuming the last
    /// process's position, and say how many fingerprints came back.
    ///
    /// `None` for a tape, which carries its own records and has no vendor to
    /// redeliver from.
    ///
    /// The failure this closes at the root: without it every restart began
    /// with an empty dedup window, so a table-shaped source republished its
    /// whole table as new observations at each rollout, and the session and
    /// span figures that make "streamed for a week" a checkable claim reset to
    /// zero at every start-up. Call it before the first [`Self::sense`]: the
    /// dedup window refuses a restore once it has observed anything, so a
    /// later call is a refusal rather than a partial restore.
    ///
    /// The store is kept so that [`Self::readmit`] re-attaches it to the
    /// connector it opens.
    pub fn journal_to(&mut self, store: Arc<dyn KeyValueStore>) -> Result<Option<usize>> {
        match self {
            Self::Tape(_) => Ok(None),
            Self::Connector { feed, journal, .. } => {
                let resumed = feed.journal_to(store.clone())?;
                *journal = Some(store);
                Ok(Some(resumed))
            }
        }
    }

    /// The source this feed's connector opens, or `None` for a tape.
    ///
    /// Read by the approval route to decide whether a registration just
    /// recorded is about the source this process actually senses. A source
    /// that is not the configured one has no connector here to re-open, and
    /// saying so is not the same as saying the re-admission failed.
    pub fn connector_source(&self) -> Option<&str> {
        match self {
            Self::Tape(_) => None,
            Self::Connector { settings, .. } => Some(&settings.source_id),
        }
    }

    /// Re-run the admission gate against `registrations` and replace this
    /// connector with the one it admits.
    ///
    /// The failure this closes: the gate ran once, at start-up, against the
    /// registry the deployment's configuration stood for. An operator who
    /// approved a registration afterwards moved the registry and the event
    /// log and nothing else — the process went on refusing the source it had
    /// refused at boot, and the only cure was a restart nobody was told to
    /// make.
    ///
    /// Both gates are re-run, not just the registration one: the licensing
    /// catalogue is read again and the source is re-opened through it, so a
    /// registration can never be the thing that admits a source whose terms
    /// still refuse it. `self` is left exactly as it was on any refusal — the
    /// replacement is the last step and a feed that failed to re-open keeps
    /// serving the cycle it was already serving.
    /// The stream journal is re-attached to the replacement, because the
    /// replacement is the same stream. A re-admission that dropped it would
    /// leave the process streaming with no durable record from the first
    /// operator approval onwards, and the ledger would read as a stream that
    /// simply stopped — the journal resumes from the checkpoint the previous
    /// connector committed, so the dedup window survives the swap too and the
    /// source is not republished wholesale by an approval.
    pub fn readmit(&mut self, registrations: &RegistrationRegistry, at: Timestamp) -> Result<()> {
        let (settings, seed, journal) = match self {
            Self::Tape(_) => {
                return Err(Error::invalid(
                    "this process senses a tape, not a connector, so there is no source to \
                     re-admit; a tape carries its own records and no registration gates it",
                ));
            }
            Self::Connector {
                settings,
                seed,
                journal,
                ..
            } => (settings.clone(), *seed, journal.clone()),
        };
        let mut replacement = Self::connector_registered(&settings, registrations, seed, at)?;
        if let Some(store) = journal {
            replacement.journal_to(store)?;
        }
        *self = replacement;
        Ok(())
    }

    fn adapter_mut(&mut self) -> &mut dyn DataAdapter {
        match self {
            Self::Tape(feed) => feed.as_mut(),
            Self::Connector { feed, .. } => feed.as_mut(),
        }
    }

    pub fn descriptor(&self) -> SourceDescriptor {
        match self {
            Self::Tape(feed) => feed.descriptor(),
            Self::Connector { feed, .. } => feed.descriptor(),
        }
    }

    /// The clock this source owns, if it owns one.
    ///
    /// A tape does; a connector runs on the wall clock. The platform's
    /// `Context` must be built on the clock returned here, or the platform
    /// prices every opportunity as of today while observing last year — and
    /// the cost router, asked for a latency budget that ended months ago,
    /// declines to convene anything.
    pub fn owned_clock(&self) -> Option<Arc<ManualClock>> {
        match self {
            Self::Tape(feed) => Some(feed.clock()),
            Self::Connector { .. } => None,
        }
    }

    /// The standing gate's own account of itself, for the banner.
    ///
    /// `None` for a tape. Printed rather than kept private because the number
    /// that distinguishes a gate consulted on every cycle from one consulted at
    /// start-up is its check count, and an operator who cannot read it can only
    /// believe the claim.
    pub fn licensing_standing(&self) -> Option<String> {
        match self {
            Self::Tape(_) => None,
            Self::Connector { admission, .. } => Some(admission.describe()),
        }
    }

    /// The gate's decision, for a connector.
    pub fn licensing_decision(&self) -> Option<&LicensingDecision> {
        match self {
            Self::Tape(_) => None,
            Self::Connector { decision, .. } => Some(decision.as_ref()),
        }
    }

    /// Whether this source has nothing left to give. A connector stops
    /// answering; it does not run out.
    pub fn is_exhausted(&self) -> bool {
        match self {
            Self::Tape(feed) => feed.remaining() == 0,
            Self::Connector { .. } => false,
        }
    }

    /// Refuse a tape that outlasts the organisation's authorisation.
    ///
    /// The platform stamps every manifest reviewed at assembly, which on a
    /// tape is the tape's first instant, and refuses to run an agent once
    /// `now` reaches the review interval. A tape longer than the interval
    /// therefore runs its remaining periods with every panel refused — a
    /// 320-day daily tape once convened its first panel on tape day 103 and
    /// reported every agent `failed` on every panel after, which read as an
    /// agent defect and was governance working as designed. Refused at
    /// start-up instead, by asking the assembled organisation itself whether
    /// it would still be authorised at the tape's last instant, so this root
    /// does not carry a second copy of the roster's review rule. A source
    /// that owns no clock has nothing to check.
    pub fn refuse_tape_beyond_authorisation(&self, platform: &Platform) -> Result<()> {
        let Self::Tape(feed) = self else {
            return Ok(());
        };
        let Some((first, last)) = feed.tape().span() else {
            return Ok(());
        };
        let lapsed: Vec<String> = platform
            .organisation()
            .review_governance(last)
            .into_iter()
            .filter(|finding| finding.severity == qip_agents::governance::Severity::Error)
            .map(|finding| format!("{}: {}", finding.rule, finding.detail))
            .collect();
        if lapsed.is_empty() {
            return Ok(());
        }
        Err(Error::invalid(format!(
            "the tape runs from {} to {}, and by its last period the organisation would refuse \
             to run: {}. Shorten the tape or use a finer interval; a roster cannot be \
             re-reviewed inside a replay",
            first.to_rfc3339(),
            last.to_rfc3339(),
            lapsed.join("; ")
        )))
    }

    /// One line for the banner: what the source is and what it is not.
    pub fn describe(&self) -> String {
        match self {
            Self::Tape(feed) => {
                let tape = feed.tape();
                let span = tape.span().map_or_else(
                    || "an empty span".to_string(),
                    |(first, last)| format!("{} to {}", first.to_rfc3339(), last.to_rfc3339()),
                );
                format!(
                    "{}: {} observation(s) across {} instrument(s) in {} period(s), {span}; \
                     tape time drives the platform clock, one period per POST /cycle, and the \
                     tape is NOT production-grade — no capital decision may rest on it",
                    feed.descriptor().name,
                    tape.len(),
                    tape.instruments().len(),
                    tape.periods()
                )
            }
            Self::Connector { feed, decision, .. } => format!(
                "connector {} ({}), {}; licensing: {}",
                feed.descriptor().name,
                feed.descriptor().provider,
                if feed.descriptor().is_production_grade() {
                    "production-grade"
                } else {
                    "NOT production-grade"
                },
                decision.describe()
            ),
        }
    }

    /// Advance to the next cycle instant and pull everything knowable by it,
    /// validating as it goes.
    ///
    /// For a tape the instant is the next knowable period and the tape's
    /// clock is moved to it; for a connector it is `wall`. A spent tape is a
    /// refusal, not an empty batch: the caller checks
    /// [`Self::is_exhausted`] first and answers the request accordingly.
    /// A connector's licensing gate is re-asked at `at` **before** the socket,
    /// and a refusal returns here rather than producing an empty batch: an
    /// empty batch and a source this process is no longer licensed to read are
    /// indistinguishable to every route downstream, and the second must refuse
    /// the cycle rather than quietly starve it. Evaluation *then* use is a rule
    /// about every use, not about the first one.
    pub fn sense(&mut self, wall: Timestamp) -> Result<Sensed> {
        let at = match self {
            Self::Tape(feed) => feed.advance().ok_or_else(|| {
                Error::unavailable(
                    "the tape is spent; every period has been released and there is no next \
                     instant to cycle at. Restart the process to replay it",
                )
            })?,
            Self::Connector { admission, .. } => {
                admission.check(wall)?;
                wall
            }
        };
        let source = self.descriptor().name;
        let mut sensed = Sensed {
            source: source.clone(),
            at,
            records: Vec::new(),
            rejections: Vec::new(),
            digest: None,
        };
        for record in self.adapter_mut().poll(at)? {
            let issues = record.validate();
            if issues.is_empty() {
                sensed.records.push(record);
            } else {
                sensed.rejections.push(format!(
                    "{source} produced an unusable {}: {}",
                    record.topic().name(),
                    issues.join("; ")
                ));
            }
        }
        if let Self::Connector { feed, .. } = self {
            sensed.digest = feed.take_digest();
        }
        Ok(sensed)
    }

    /// The current instant on whichever clock this source runs on.
    pub fn now(&self, wall: &dyn Clock) -> Timestamp {
        match self.owned_clock() {
            Some(clock) => clock.now(),
            None => wall.now(),
        }
    }
}
