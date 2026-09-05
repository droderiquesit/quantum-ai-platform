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
//!   catalogue has admitted it. [`ApiFeed::connector`] is shaped so there is
//!   no route to a connector around the gate.
//!
//! A tape and a connector at once is a contradiction rather than a
//! precedence question: whichever this code preferred, the operator meant the
//! other one somewhere, and the only answer that cannot be wrong is a refusal
//! that names both variables.

use qip_core::error::{Error, Result};
use qip_core::{Clock, ManualClock, Timestamp};
use qip_data_finder::admission::{self, CatalogueEntry, LicensingDecision};
use qip_kernel::Platform;
use qip_market_ingestion::adapter::{DataAdapter, SensedRecord, SourceDescriptor};
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
        decision: LicensingDecision,
    },
}

impl ApiFeed {
    /// Open whichever source the settings name, or none.
    ///
    /// `at` is the instant a connector is admitted and connected at; a tape
    /// starts on its own first knowable instant and ignores it.
    pub fn open(settings: &FeedSettings, seed: u64, at: Timestamp) -> Result<Option<Self>> {
        match settings {
            FeedSettings::None => Ok(None),
            FeedSettings::Tape(path) => Self::tape(path).map(Some),
            FeedSettings::Connector(connector) => Self::connector(connector, seed, at).map(Some),
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
    /// licensing gate has admitted it.
    pub fn connector(settings: &ConnectorSettings, seed: u64, at: Timestamp) -> Result<Self> {
        Self::connector_admitted_by(&admission::catalogue()?, settings, seed, at)
    }

    /// The same opening against a caller-supplied licensing catalogue.
    ///
    /// The gate runs here, before anything is constructed and before any
    /// socket is touched: the rule is evaluation *then* use, and putting the
    /// call inside the constructor makes the ordering a property of the code
    /// path rather than of the caller's memory. Split from [`Self::connector`]
    /// so a test can hold the gate against an entry the real catalogue must
    /// never contain and prove that no socket opens.
    pub fn connector_admitted_by(
        entries: &[CatalogueEntry],
        settings: &ConnectorSettings,
        seed: u64,
        at: Timestamp,
    ) -> Result<Self> {
        let class = shipped_class(&settings.source_id)?;
        let decision = admission::admit_from(entries, &settings.source_id, class, at)?;
        let feed = ConnectorFeed::open(&settings.source_id, &settings.base_url, seed, at)?;
        Ok(Self::Connector {
            feed: Box::new(feed),
            decision,
        })
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

    /// The gate's decision, for a connector.
    pub fn licensing_decision(&self) -> Option<&LicensingDecision> {
        match self {
            Self::Tape(_) => None,
            Self::Connector { decision, .. } => Some(decision),
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
            Self::Connector { feed, decision } => format!(
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
    pub fn sense(&mut self, wall: Timestamp) -> Result<Sensed> {
        let at = match self {
            Self::Tape(feed) => feed.advance().ok_or_else(|| {
                Error::unavailable(
                    "the tape is spent; every period has been released and there is no next \
                     instant to cycle at. Restart the process to replay it",
                )
            })?,
            Self::Connector { .. } => wall,
        };
        let source = self.descriptor().name;
        let mut sensed = Sensed {
            source: source.clone(),
            at,
            records: Vec::new(),
            rejections: Vec::new(),
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
