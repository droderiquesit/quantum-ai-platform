//! The breadth plane: the Market Factory, event markets and physical
//! commerce, installed from one deployment plan and run inside the cycle
//! (R12-EXEC-5080/-5081, R12-COVERAGE-5107, R12-EVENT-5082 to -5089,
//! R12-COMMERCE-5090 to -5097).
//!
//! All three were libraries nothing called. A composition root now reads a
//! [`BreadthPlan`] and hands it to [`Platform::install_breadth`]; from then
//! on DISCOVER polls the event and commerce sources and SIMULATE prices,
//! paper-trades, and runs the Market Factory's validity monitor, each
//! journaling what it did to the hash-chained event log in the same pass.
//!
//! # Off unless a plan says otherwise
//!
//! A platform with no plan installed runs exactly as before: every hook
//! below returns at its first line. A plan section that cannot be honoured
//! — a source other than the synthetic ones, a permission that does not
//! parse, a market the register does not admit, a broker that offers no
//! listing venue — refuses the whole installation naming the section, so a
//! deployment that asked for breadth and got half of it stops at start-up.
//!
//! # Nothing is created without an admission
//!
//! Every market and instrument the plane brings into existence rests on the
//! three-authority register of `qip_execution_engine::creation` — venue,
//! product and jurisdiction — read from the plan through the same
//! [`crate::market_factory_stage::register`] for both sections that create
//! one. The Market Factory consumes an admission per step; the event plane's
//! own market and each of its outcome instruments need one for `create`
//! before the synthetic venue that holds them is built, and the admissions
//! are journaled with their references (R12-COVERAGE-5107). Until
//! 2026-10-24 the event plane built its venue, and so its market and three
//! instruments, with no admission of any kind.
//!
//! # Paper, by structure
//!
//! The Market Factory lists only through the desk broker's listing venue,
//! which only the simulated broker offers. Event and commerce positions live
//! in their planes' own paper books; nothing here reaches the order manager,
//! capital, or sizing. Both sources are synthetic and research-only, and
//! their records say so.

use std::collections::BTreeMap;

use qip_contracts::VenueId;
use qip_core::error::{Error, Result};
use qip_core::{Duration, Timestamp};
use qip_events::{EventBody, Topic};
use qip_execution_engine::creation::{CreationAction, CreationAdmission, CreationRequest};
use qip_execution_engine::market_creation::PricingRule;
use qip_prediction::access::{AccessBook, EntityRecord, Jurisdiction, VenuePolicy};
use qip_prediction::{
    DEMO_MARKET_ID, DEMO_OUTCOME_INSTRUMENTS, OracleIdentity, OracleKind, SyntheticPredictionVenue,
    SyntheticVenueConfig,
};
use serde::{Deserialize, Serialize};

use crate::commerce_research::{CommercePlan, CommerceResearch, SyntheticCommerceFeed};
use crate::cycle::StageOutcome;
use crate::event_markets::EventMarketPlane;
use crate::market_factory_stage::{FactoryPlan, FactoryStage, PermissionEntry, register};
use crate::platform::Platform;
use qip_observability::metrics::labels;

/// Market Factory permission-register revisions, by `outcome`: `applied` or
/// `revoked_all` (the fail-closed answer to an unreadable register).
pub const MARKET_PERMISSIONS_REVISED: &str = "qip_market_permissions_revised_total";

/// The only source name this build can open for either plane.
pub const SYNTHETIC_SOURCE: &str = "synthetic";

/// The event-markets section of a breadth plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventMarketPlan {
    /// `synthetic` is the only source this build can open.
    pub source: String,
    /// The entity paper orders are admitted for.
    pub entity: String,
    /// Venue policies for the access book. Empty refuses every order.
    #[serde(default)]
    pub policies: Vec<VenuePolicy>,
    /// Entity records for the access book. Empty refuses every order.
    #[serde(default)]
    pub entities: BTreeMap<String, EntityRecord>,
    /// The jurisdiction the plane's own market and its outcome instruments
    /// are created in, matched exactly against `permissions`. Defaulted only
    /// so a section that omits it parses and is then refused at
    /// installation with the reason, rather than failing to parse: an empty
    /// jurisdiction matches no recorded permission.
    #[serde(default)]
    pub jurisdiction: String,
    /// The creation register for the plane's own market and each outcome
    /// instrument: the same entries, and the same three authorities, the
    /// Market Factory's section records. Each instrument needs `create` from
    /// the venue, the product rules and the jurisdiction. Empty refuses the
    /// section (R12-COVERAGE-5107).
    #[serde(default)]
    pub permissions: Vec<PermissionEntry>,
}

/// One instrument of the event plane's own market admitted for creation, as
/// the event log holds it: on whose word the market and each instrument the
/// synthetic venue quotes came to exist.
///
/// Journaled because an admission that gated the install and was then
/// dropped would leave the log unable to say why a market exists — the
/// Market Factory's records carry their approvals for the same reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventMarketCreationAdmitted {
    pub venue: String,
    pub product: String,
    pub jurisdiction: String,
    /// `authority: reference`, one per authority, in authority order.
    pub approvals: Vec<String>,
}

impl EventMarketCreationAdmitted {
    fn from_admission(admission: &CreationAdmission) -> Self {
        let request = admission.request();
        Self {
            venue: request.venue.clone(),
            product: request.product.clone(),
            jurisdiction: request.jurisdiction.clone(),
            approvals: admission
                .references()
                .iter()
                .map(|(authority, reference)| format!("{}: {reference}", authority.as_str()))
                .collect(),
        }
    }
}

impl EventBody for EventMarketCreationAdmitted {
    /// A governed product decision with its approval references, like the
    /// Market Factory's: only this platform has it.
    const TOPIC: Topic = Topic::ComplianceEvaluated;
    const SCHEMA_VERSION: u32 = 1;

    fn idempotency_key(&self) -> Option<String> {
        Some(format!(
            "event-market-creation:{}:{}",
            self.venue, self.product
        ))
    }
}

/// What a deployment asks the breadth plane to run. Every section optional.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreadthPlan {
    #[serde(default)]
    pub market_factory: Option<FactoryPlan>,
    #[serde(default)]
    pub event_markets: Option<EventMarketPlan>,
    #[serde(default)]
    pub commerce: Option<CommercePlan>,
}

impl BreadthPlan {
    /// Parse a plan, refusing unknown fields: a misspelt section is a
    /// section silently not run.
    pub fn from_json(text: &str) -> Result<Self> {
        serde_json::from_str(text).map_err(|error| {
            Error::invalid(format!(
                "configuration: the breadth plan does not parse: {error}"
            ))
        })
    }

    pub fn is_empty(&self) -> bool {
        self.market_factory.is_none() && self.event_markets.is_none() && self.commerce.is_none()
    }
}

/// What each plane holds once installed. Empty on every platform until a
/// plan is installed.
#[derive(Debug, Default)]
pub struct BreadthPlane {
    factory: Option<FactoryStage>,
    events: Option<EventMarketPlane>,
    commerce: Option<CommerceResearch>,
}

impl BreadthPlane {
    pub fn is_idle(&self) -> bool {
        self.factory.is_none() && self.events.is_none() && self.commerce.is_none()
    }
}

/// The oracle the synthetic event venue's reports are judged against: an
/// optimistic oracle, challengeable for two hours, whose report is refused
/// below nine-tenths confidence.
fn synthetic_oracle() -> Result<OracleIdentity> {
    OracleIdentity::new(
        "synthetic-prediction-oracle",
        OracleKind::Optimistic,
        Duration::from_hours(2),
        0.9,
    )
}

/// The access book an event-markets section describes. Empty refuses
/// every paper order, and the creation of every Market Factory market
/// flagged `event_contract` (R12-EVENT-5087), which it gates by venue and
/// jurisdiction.
///
/// It does not gate the event plane's own market. This said an empty book
/// refused "every creation" until 2026-10-24, while the plane's market and
/// its three outcome instruments were built with no admission at all. Those
/// are admitted by the section's `permissions` register, in
/// [`admit_event_market`], and by nothing in this book.
fn access_book(section: &EventMarketPlan) -> AccessBook {
    let mut access = AccessBook::new();
    for policy in &section.policies {
        access.set_policy(policy.clone());
    }
    for (id, record) in &section.entities {
        access.set_entity(id, record.clone());
    }
    access
}

/// The kill-switch scope a Market Factory ladder is admitted under, so an
/// operator can halt market making without halting the desk.
pub const MARKET_FACTORY_SCOPE: &str = "market-factory";

/// Admit the event plane's own market and every outcome instrument its
/// venue quotes for `create` at `venue` in the section's jurisdiction, from
/// the section's register — or refuse, naming every instrument refused.
///
/// Called before the synthetic venue is built, because building it is what
/// brings [`DEMO_MARKET_ID`] and its instruments into existence; the ids
/// are read from `qip_prediction`'s constants, which are the ones the venue
/// builds them from. Every instrument is asked about before any refusal is
/// returned, as `CreationPermissions::admit` names every missing authority:
/// a refusal that named the first would make four admissions a sequence of
/// four refusals.
fn admit_event_market(
    section: &EventMarketPlan,
    venue: &VenueId,
) -> Result<Vec<EventMarketCreationAdmitted>> {
    let permissions = register(&section.permissions).map_err(|error| {
        Error::invalid(format!(
            "configuration: the event-markets section's permissions do not parse: {}",
            error.message()
        ))
    })?;
    let mut admitted = Vec::new();
    let mut refused = Vec::new();
    for product in std::iter::once(DEMO_MARKET_ID).chain(DEMO_OUTCOME_INSTRUMENTS) {
        let request = CreationRequest::new(
            CreationAction::Create,
            venue.as_str(),
            product,
            section.jurisdiction.as_str(),
        );
        match permissions.admit(&request) {
            Ok(admission) => admitted.push(EventMarketCreationAdmitted::from_admission(&admission)),
            Err(error) => refused.push(format!("{product}: {}", error.message())),
        }
    }
    if !refused.is_empty() {
        return Err(Error::denied(format!(
            "configuration: the event-markets section's market {DEMO_MARKET_ID} and its outcome \
             instruments may be created at {venue} only under a recorded venue, product and \
             jurisdiction permission to create each, and {} of {} are not admitted in \
             jurisdiction `{}`; record them under the section's `permissions`. No venue, market \
             or instrument was created: {}",
            refused.len(),
            refused.len() + admitted.len(),
            section.jurisdiction,
            refused.join("; ")
        )));
    }
    Ok(admitted)
}

/// The event plane a section describes, the admissions its market was
/// created under, and its banner line. Builds nothing the platform shares:
/// the venue is in-memory and synthetic, so a refusal here or later leaves
/// nothing behind once the value is dropped.
fn open_event_plane(
    section: &EventMarketPlan,
    access: AccessBook,
    seed: u64,
    now: Timestamp,
) -> Result<(EventMarketPlane, Vec<EventMarketCreationAdmitted>, String)> {
    if section.source != SYNTHETIC_SOURCE {
        return Err(refuse_source("event-market", &section.source));
    }
    let config = SyntheticVenueConfig::demo(seed)?;
    let admitted = admit_event_market(section, &config.venue)?;
    let venue = SyntheticPredictionVenue::new(config, now)?;
    let plane = EventMarketPlane::open(
        Box::new(venue),
        None,
        access,
        section.entity.clone(),
        synthetic_oracle()?,
    )?;
    let line = format!(
        "event markets from {} ({}, research only): {} instrument(s) admitted for creation in {}; \
         paper orders admitted for {} through {} venue polic(ies)",
        plane.source().name,
        plane.posture().as_str(),
        admitted.len(),
        section.jurisdiction,
        section.entity,
        section.policies.len()
    );
    Ok((plane, admitted, line))
}

fn refuse_source(section: &str, source: &str) -> Error {
    Error::invalid(format!(
        "configuration: the breadth plan's {section} source is `{source}`, and only \
         `{SYNTHETIC_SOURCE}` can be opened in this build: no real {section} source has a \
         licensing evaluation, and one is refused before it is polled"
    ))
}

impl Platform {
    /// Install a breadth plan, refusing it whole if any section cannot be
    /// honoured. Returns one line per installed section, for the banner.
    pub fn install_breadth(&mut self, plan: &BreadthPlan, now: Timestamp) -> Result<Vec<String>> {
        if !self.breadth.is_idle() {
            return Err(Error::invalid(
                "a breadth plan is already installed; a second would run beside the first",
            ));
        }
        let seed = self.config().seed;
        let access = plan.event_markets.as_ref().map(access_book);
        // The event and commerce planes are built first. Both are in-memory
        // planes over synthetic sources, so a refusal in either — a source
        // with no licensing evaluation, a market its register does not
        // admit — returns before anything the platform shares has been
        // touched. The factory goes last because it is the one section that
        // writes to a venue the platform shares: it lists and seeds on the
        // desk broker's simulated venue. Until 2026-10-24 it went first, so
        // an event section refused after it left the factory's books resting
        // on that venue for a plan the caller was told was refused.
        let events = match &plan.event_markets {
            Some(section) => Some(open_event_plane(
                section,
                access.clone().unwrap_or_default(),
                seed,
                now,
            )?),
            None => None,
        };
        let commerce = match &plan.commerce {
            Some(section) => {
                if section.source != SYNTHETIC_SOURCE {
                    return Err(refuse_source("commerce", &section.source));
                }
                let research = CommerceResearch::open(
                    Box::new(SyntheticCommerceFeed::new(seed, now)),
                    None,
                    section.clone(),
                )?;
                let line = format!(
                    "commerce research from {} ({}, research only): purchases through account {} \
                     under a ceiling of {}",
                    research.source().name,
                    research.posture().as_str(),
                    section.account.id,
                    section.total_ceiling
                );
                Some((research, line))
            }
            None => None,
        };
        let factory = match &plan.market_factory {
            Some(section) => {
                // R12-EVENT-5087: an event market is created only where the
                // access book permits creation from its jurisdiction, and
                // the check comes before the factory touches the venue.
                for market in section
                    .markets
                    .iter()
                    .filter(|market| market.event_contract)
                {
                    let book = access.as_ref().ok_or_else(|| {
                        Error::invalid(format!(
                            "configuration: market {} is an event contract and the plan has no \
                             event-markets section, so no access book could permit its creation",
                            market.product
                        ))
                    })?;
                    book.admit_creation(
                        &VenueId::new(market.venue.as_str()),
                        &Jurisdiction::new(&market.jurisdiction)?,
                    )
                    .map_err(|error| {
                        Error::invalid(format!(
                            "configuration: event market {} refused: {}",
                            market.product,
                            error.message()
                        ))
                    })?;
                }
                // R12-RISK-4093: a seeded ladder is orders at a venue, so each
                // is measured against the desk's limits over the book as it
                // stands — working orders included — before it rests, by the
                // same order manager every desk order passes through. Taken
                // before the venue is borrowed, because the book is the
                // platform's and the venue is the broker's.
                let risk_state = self.risk_state();
                let orders = &mut self.orders;
                let autonomy = &self.autonomy;
                let mut ladders = |product: &str, venue: &str, rule: &PricingRule| {
                    orders.admit_ladder(
                        product,
                        venue,
                        rule,
                        MARKET_FACTORY_SCOPE,
                        autonomy,
                        &risk_state,
                        now,
                    )
                };
                let venue = self.broker.listing_venue().ok_or_else(|| {
                    Error::denied(
                        "configuration: the breadth plan asks for a market factory and the desk \
                         broker offers no listing venue; markets are created on the simulated \
                         venue only",
                    )
                })?;
                let stage =
                    FactoryStage::install(section, venue, &mut ladders, now).map_err(|error| {
                        Error::invalid(format!(
                            "configuration: the market factory refused the plan: {}",
                            error.message()
                        ))
                    })?;
                let line = format!(
                    "market factory at {}: {} market(s) created, listed and seeded",
                    stage.factory().venue(),
                    stage.factory().markets().count()
                );
                Some((stage, line))
            }
            None => None,
        };
        // Banner lines in the plan's own section order, whatever order the
        // sections were built in.
        let mut lines = Vec::new();
        let factory = factory.map(|(stage, line)| {
            lines.push(line);
            stage
        });
        let (events, created) = match events {
            Some((plane, created, line)) => {
                lines.push(line);
                (Some(plane), created)
            }
            None => (None, Vec::new()),
        };
        let commerce = commerce.map(|(research, line)| {
            lines.push(line);
            research
        });
        self.breadth = BreadthPlane {
            factory,
            events,
            commerce,
        };
        // Every creation reaches the log before anything else runs, so a
        // market exists only with the record of what admitted it: the event
        // plane's admissions, then the factory's installation steps.
        for admission in created {
            self.journal_record(admission, "kernel/event-markets", now)?;
        }
        let records = self
            .breadth
            .factory
            .as_mut()
            .map(FactoryStage::drain)
            .unwrap_or_default();
        for record in records {
            self.journal_record(record, "kernel/market-factory", now)?;
        }
        Ok(lines)
    }

    /// The book the desk's simulated venue rests for `object_id`, as the
    /// Market Factory listed it — `None` when nothing is listed there or the
    /// broker is not one a factory may list on. A copy, so nothing reaches
    /// the venue through it. Without it no caller could see whether a market
    /// the factory journaled as seeded was ever put on the venue, and a test
    /// of seeding that read the factory's own record passed with the venue
    /// holding nothing (R12-EXEC-5080). `&mut self` only because the broker
    /// exposes its listing venue mutably.
    pub fn listed_book(&mut self, object_id: &str) -> Option<qip_market::book::OrderBook> {
        self.broker
            .listing_venue()
            .and_then(|venue| venue.listed_book(object_id).cloned())
    }

    /// The Market Factory, when a plan installed one.
    pub fn market_factory(&self) -> Option<&FactoryStage> {
        self.breadth.factory.as_ref()
    }

    /// The Market Factory, for an operator replacing its permission register.
    pub fn market_factory_mut(&mut self) -> Option<&mut FactoryStage> {
        self.breadth.factory.as_mut()
    }

    /// Replace the Market Factory's permission register with `entries`, as
    /// the deep brain does each cycle from its plan file (R12-EXEC-5081).
    ///
    /// The next SIMULATE review judges every live market against the new
    /// register, and one whose create, list or seed permission is gone is
    /// flagged and withdrawn. That is the only path from an operator's
    /// revocation to a withdrawn market; before it the register was fixed
    /// at install. Refuses with no factory installed, and refuses a
    /// register that does not parse, leaving the old one in place, so the
    /// caller decides how to fail; the deep brain revokes everything.
    /// Returns the number of entries applied.
    pub fn revise_market_permissions(
        &mut self,
        entries: &[crate::market_factory_stage::PermissionEntry],
    ) -> Result<usize> {
        let register = crate::market_factory_stage::register(entries)?;
        let stage = self.breadth.factory.as_mut().ok_or_else(|| {
            Error::invalid("no market factory is installed, so there is no register to revise")
        })?;
        stage.set_permissions(register);
        self.telemetry
            .metrics
            .count(MARKET_PERMISSIONS_REVISED, labels([("outcome", "applied")]));
        Ok(entries.len())
    }

    /// Revoke every Market Factory permission. Every live market lapses and
    /// is withdrawn at the next review. This is the fail-closed answer to a
    /// register nobody can read. A no-op without a factory.
    pub fn revoke_market_permissions(&mut self) {
        if let Some(stage) = self.breadth.factory.as_mut() {
            stage.set_permissions(qip_execution_engine::creation::CreationPermissions::new());
            self.telemetry.metrics.count(
                MARKET_PERMISSIONS_REVISED,
                labels([("outcome", "revoked_all")]),
            );
        }
    }

    /// The event-market plane, when a plan installed one.
    pub fn event_markets(&self) -> Option<&EventMarketPlane> {
        self.breadth.events.as_ref()
    }

    /// The event-market plane, for an operator replacing its access book.
    pub fn event_markets_mut(&mut self) -> Option<&mut EventMarketPlane> {
        self.breadth.events.as_mut()
    }

    /// The commerce research plane, when a plan installed one.
    pub fn commerce_research(&self) -> Option<&CommerceResearch> {
        self.breadth.commerce.as_ref()
    }

    /// DISCOVER's breadth half: poll the event and commerce sources.
    pub(crate) fn breadth_discover(&mut self, now: Timestamp, outcome: &mut StageOutcome) {
        if self.breadth.is_idle() {
            return;
        }
        if let Some(plane) = self.breadth.events.as_mut() {
            match plane.discover(now) {
                Ok(found) => {
                    outcome.produced += found.opportunities.len();
                    outcome.detail.push_str(&format!(
                        "; event markets: {} listed, {} book(s), {} set arbitrage(s)",
                        found.listed.len(),
                        found.books,
                        found.opportunities.len()
                    ));
                    outcome.problems.extend(found.problems.iter().cloned());
                    self.journal_breadth(found, "kernel/event-markets", now, outcome);
                }
                Err(error) => outcome
                    .problems
                    .push(format!("event markets: {}", error.message())),
            }
        }
        if let Some(research) = self.breadth.commerce.as_mut() {
            match research.discover(now) {
                Ok(found) => {
                    outcome.produced += found.candidates.len();
                    outcome.detail.push_str(&format!(
                        "; commerce: {} observation(s), {} candidate(s), {} counterfeit",
                        found.observations,
                        found.candidates.len(),
                        found.counterfeit.len()
                    ));
                    outcome.problems.extend(found.problems.iter().cloned());
                    self.journal_breadth(found, "kernel/commerce", now, outcome);
                }
                Err(error) => outcome
                    .problems
                    .push(format!("commerce: {}", error.message())),
            }
        }
    }

    /// SIMULATE's breadth half: price and paper-trade event markets, run the
    /// commerce pipeline, and review every created market's validity.
    pub(crate) fn breadth_simulate(&mut self, now: Timestamp, outcome: &mut StageOutcome) {
        if self.breadth.is_idle() {
            return;
        }
        if let Some(plane) = self.breadth.events.as_mut() {
            match plane.simulate(now) {
                Ok(run) => {
                    outcome.produced += run.valuations.len();
                    outcome.detail.push_str(&format!(
                        "; event markets: {} priced, {} paper fill(s), {} quote(s), {} refused",
                        run.valuations.len(),
                        run.fills.len(),
                        run.quotes.len(),
                        run.refusals.len()
                    ));
                    outcome.problems.extend(run.problems.iter().cloned());
                    self.journal_breadth(run, "kernel/event-markets", now, outcome);
                }
                Err(error) => outcome
                    .problems
                    .push(format!("event markets: {}", error.message())),
            }
        }
        if let Some(research) = self.breadth.commerce.as_mut() {
            match research.simulate() {
                Ok(run) => {
                    outcome.produced += run.assessed.len();
                    outcome.detail.push_str(&format!(
                        "; commerce: {} assessed, {} purchase(s), {} listed, {} sold",
                        run.assessed.len(),
                        run.purchases.len(),
                        run.listed.len(),
                        run.sold.len()
                    ));
                    outcome.problems.extend(run.problems.iter().cloned());
                    self.journal_breadth(run, "kernel/commerce", now, outcome);
                }
                Err(error) => outcome
                    .problems
                    .push(format!("commerce: {}", error.message())),
            }
        }
        if self.breadth.factory.is_some() {
            let marks = self
                .breadth
                .factory
                .as_ref()
                .map(|stage| stage.marks(self.price_history()))
                .unwrap_or_default();
            let reviewed = match (self.broker.listing_venue(), self.breadth.factory.as_mut()) {
                (Some(venue), Some(stage)) => stage.review(&marks, venue, now),
                _ => Err(Error::denied(
                    "the desk broker no longer offers a listing venue, so no created market \
                     could be reviewed",
                )),
            };
            match reviewed {
                Ok(findings) => {
                    outcome.detail.push_str(&format!(
                        "; market factory: {} market(s) withdrawn",
                        findings.len()
                    ));
                }
                Err(error) => outcome
                    .problems
                    .push(format!("market factory: {}", error.message())),
            }
            let records = self
                .breadth
                .factory
                .as_mut()
                .map(FactoryStage::drain)
                .unwrap_or_default();
            for record in records {
                self.journal_breadth(record, "kernel/market-factory", now, outcome);
            }
        }
    }

    fn journal_breadth<B: EventBody>(
        &mut self,
        body: B,
        origin: &str,
        now: Timestamp,
        outcome: &mut StageOutcome,
    ) {
        if let Err(error) = self.journal_record(body, origin, now) {
            outcome.problems.push(format!(
                "{origin}: a record was computed and not journalled: {}",
                error.message()
            ));
        }
    }
}
