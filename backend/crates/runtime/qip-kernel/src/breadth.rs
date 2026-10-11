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
use qip_events::EventBody;
use qip_execution_engine::market_creation::PricingRule;
use qip_prediction::access::{AccessBook, EntityRecord, Jurisdiction, VenuePolicy};
use qip_prediction::{OracleIdentity, OracleKind, SyntheticPredictionVenue, SyntheticVenueConfig};
use serde::{Deserialize, Serialize};

use crate::commerce_research::{CommercePlan, CommerceResearch, SyntheticCommerceFeed};
use crate::cycle::StageOutcome;
use crate::event_markets::EventMarketPlane;
use crate::market_factory_stage::{FactoryPlan, FactoryStage};
use crate::platform::Platform;

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
/// every order and every creation.
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
        let mut lines = Vec::new();
        let access = plan.event_markets.as_ref().map(access_book);
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
                lines.push(format!(
                    "market factory at {}: {} market(s) created, listed and seeded",
                    stage.factory().venue(),
                    stage.factory().markets().count()
                ));
                Some(stage)
            }
            None => None,
        };
        let events = match &plan.event_markets {
            Some(section) => {
                if section.source != SYNTHETIC_SOURCE {
                    return Err(refuse_source("event-market", &section.source));
                }
                let access = access.clone().unwrap_or_default();
                let venue = SyntheticPredictionVenue::new(SyntheticVenueConfig::demo(seed)?, now)?;
                let plane = EventMarketPlane::open(
                    Box::new(venue),
                    None,
                    access,
                    section.entity.clone(),
                    synthetic_oracle()?,
                )?;
                lines.push(format!(
                    "event markets from {} ({}, research only): paper orders admitted for {} \
                     through {} venue polic(ies)",
                    plane.source().name,
                    plane.posture().as_str(),
                    section.entity,
                    section.policies.len()
                ));
                Some(plane)
            }
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
                lines.push(format!(
                    "commerce research from {} ({}, research only): purchases through account {} \
                     under a ceiling of {}",
                    research.source().name,
                    research.posture().as_str(),
                    section.account.id,
                    section.total_ceiling
                ));
                Some(research)
            }
            None => None,
        };
        self.breadth = BreadthPlane {
            factory,
            events,
            commerce,
        };
        // The factory's installation steps reach the log before anything
        // else runs, so a market exists on the venue only with its record.
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

    /// The Market Factory, when a plan installed one.
    pub fn market_factory(&self) -> Option<&FactoryStage> {
        self.breadth.factory.as_ref()
    }

    /// The Market Factory, for an operator replacing its permission register.
    pub fn market_factory_mut(&mut self) -> Option<&mut FactoryStage> {
        self.breadth.factory.as_mut()
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
