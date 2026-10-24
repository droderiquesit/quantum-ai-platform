//! Event markets as a paper research and opportunity source in DISCOVER and
//! SIMULATE (R12-EVENT-5082 to -5089).
//!
//! `qip-prediction` held definitions, resolution, beliefs, pricing, set
//! arbitrage and an access book, every one tested and none called by any
//! kernel or app. This module is the caller.
//!
//! * **DISCOVER** polls the source, records each listed market's definition
//!   into the bitemporal [`DefinitionStore`] at the instant it became
//!   knowable (5082), keeps each outcome's latest book, advances each
//!   market's [`MarketResolution`] with whatever the oracle has said —
//!   resolved is not settled until the dispute window closes (5083) — and
//!   reports every complete-set arbitrage the books hold, sized against
//!   their depth.
//! * **SIMULATE** forms an [`OutcomeDistribution`] from the books' implied
//!   bands (5084), prices every outcome from it with [`fair_values`] (5085),
//!   and then — only then, and only through the [`AccessBook`] — takes the
//!   arbitrage and rests paper quotes around fair value (5086), and settles
//!   paper positions on a market whose resolution is final.
//!
//! # Paper only, and research before decision
//!
//! Nothing here reaches an order manager, a broker or capital. Positions and
//! cash live in this module's paper book and nowhere else; no event-market
//! figure enters the desk's sizing. A source whose class is synthetic is
//! research only by [`LicensingClass::allows_production_decisions`], and
//! every record this module journals says which it was.
//!
//! # Licensing posture first
//!
//! [`EventMarketPlane::open`] refuses a source before polling it unless its
//! posture is known: a synthetic source is research-only by construction,
//! and any other source must arrive with a [`LicensingDecision`] — minted
//! only by `qip-data-finder`'s gate — that names it and permits research.
//! No prediction vendor is in the catalogue, so every real source is refused
//! today, at the door, by name.
//!
//! # Regulated wagering stays out (5088)
//!
//! Every paper order and quote is admitted by
//! [`AccessBook::admit_event_order`], the event-contract entry point: a
//! venue recorded as regulated wagering is refused on this path by the book
//! itself, as is an entity out of jurisdiction, under age, ineligible, or
//! without the venue. An empty book refuses everything.
//!
//! # Listed and quoted only while the creation register admits it
//!
//! The plane holds the three-authority creation register of
//! `qip_execution_engine::creation` its market was created under, and asks
//! it again at every act, each for the action the act performs
//! (R12-COVERAGE-5107): a market and its outcome instruments are taken in
//! at a listing only under `list`, and kept only while `create` and `list`
//! still admit every one of them; a paper quote rests on an outcome only
//! under `seed`, because resting a quote is putting liquidity into the
//! market. [`EventMarketPlane::set_creation_permissions`] replaces the
//! register, and the next pass withdraws a market whose permission lapsed.
//! Until 2026-10-24 the register was consulted once, for `create`, at
//! installation, and a revoked permission stopped nothing.
//!
//! # Pricing never reads availability (5089)
//!
//! [`EventMarketPlane::price`] takes no access book: the belief and the fair
//! value are formed before the gate is consulted, so the price of an event
//! is the same whether every venue or none is available to the entity.

use std::collections::{BTreeMap, BTreeSet};

use qip_contracts::governance::Usage;
use qip_core::error::{Error, Result};
use qip_core::{Decimal, ObjectId, Timestamp};
use qip_data_finder::admission::LicensingDecision;
use qip_events::{EventBody, Topic};
use qip_execution_engine::creation::{CreationAction, CreationPermissions, CreationRequest};
use qip_financial::quality::LicensingClass;
use qip_market::book::OrderBook;
use qip_prediction::access::AccessBook;
use qip_prediction::belief::{OutcomeDistribution, Valuation, fair_values};
use qip_prediction::{
    DefinitionStore, EventDefinition, EventMarket, MarketResolution, OracleIdentity, OutcomeId,
    PredictionAdapter, PredictionSource, PredictionUpdate, Probability, ResolutionState,
    SetArbitrageKind, implied_band, set_arbitrage,
};
use serde::{Deserialize, Serialize};

/// The most event markets one plane tracks. Bounded retention: a listing
/// past the bound is reported and dropped rather than growing the set.
pub const MAX_EVENT_MARKETS: usize = 32;

/// Half the width of a paper quote around fair value.
const PAPER_QUOTE_HALF_SPREAD: Decimal = Decimal::from_raw(10_000_000);

/// What the plane knows about its source's licensing.
#[derive(Clone, Debug)]
pub enum SourcePosture {
    /// A synthetic source: research only, never decision-grade.
    Synthetic,
    /// A real source the licensing gate admitted for research.
    Admitted(Box<LicensingDecision>),
}

impl SourcePosture {
    /// The licensing class the posture amounts to.
    pub fn class(&self) -> LicensingClass {
        match self {
            Self::Synthetic => LicensingClass::Synthetic,
            Self::Admitted(decision) => decision.class,
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Synthetic => "synthetic",
            Self::Admitted(_) => "admitted",
        }
    }
}

/// One complete-set arbitrage DISCOVER found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventOpportunity {
    pub market_id: ObjectId,
    pub kind: SetArbitrageKind,
    pub quantity: Decimal,
    pub profit: Decimal,
}

/// What one DISCOVER pass did, journaled.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EventMarketsDiscovered {
    pub source: String,
    pub posture: String,
    pub decision_grade: bool,
    pub listed: Vec<ObjectId>,
    pub books: usize,
    pub resolutions: BTreeMap<String, String>,
    pub opportunities: Vec<EventOpportunity>,
    pub problems: Vec<String>,
    /// Markets withdrawn this pass because the creation register no longer
    /// admits them, each with the refusal. Defaulted so a record journaled
    /// before the field existed still decodes.
    #[serde(default)]
    pub withdrawn: Vec<String>,
}

impl EventBody for EventMarketsDiscovered {
    const TOPIC: Topic = Topic::OpportunityDetected;
    const SCHEMA_VERSION: u32 = 1;
}

/// A paper fill on one outcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaperEventFill {
    pub market_id: ObjectId,
    pub outcome: OutcomeId,
    /// Positive bought, negative sold.
    pub quantity: Decimal,
    pub price: Decimal,
}

/// A paper quote resting around fair value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaperEventQuote {
    pub market_id: ObjectId,
    pub outcome: OutcomeId,
    pub bid: Decimal,
    pub ask: Decimal,
}

/// What one SIMULATE pass did, journaled.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EventMarketsSimulated {
    pub decision_grade: bool,
    pub valuations: Vec<Valuation>,
    pub fills: Vec<PaperEventFill>,
    pub quotes: Vec<PaperEventQuote>,
    /// Every order or quote the access book refused, with its sentence.
    pub refusals: Vec<String>,
    /// Net paper cash settled from final resolutions this pass.
    pub settled: Decimal,
    pub problems: Vec<String>,
    /// Markets withdrawn this pass because the creation register no longer
    /// admits them, each with the refusal.
    #[serde(default)]
    pub withdrawn: Vec<String>,
}

impl EventBody for EventMarketsSimulated {
    const TOPIC: Topic = Topic::ScenarioEvaluated;
    const SCHEMA_VERSION: u32 = 1;
}

/// The event-market plane.
#[derive(Debug)]
pub struct EventMarketPlane {
    adapter: Box<dyn PredictionAdapter + Send + Sync>,
    source: PredictionSource,
    posture: SourcePosture,
    access: AccessBook,
    entity: String,
    oracle: OracleIdentity,
    definitions: DefinitionStore,
    markets: BTreeMap<ObjectId, EventMarket>,
    books: BTreeMap<ObjectId, BTreeMap<OutcomeId, OrderBook>>,
    resolutions: BTreeMap<ObjectId, MarketResolution>,
    positions: BTreeMap<(ObjectId, OutcomeId), Decimal>,
    cash: Decimal,
    settled: BTreeSet<ObjectId>,
    /// The newest book instant each market's arbitrage was last taken at.
    traded_at: BTreeMap<ObjectId, Timestamp>,
    /// The creation register every listing and quote is admitted against.
    creation: CreationPermissions,
    /// The jurisdiction the plane's markets were created in.
    jurisdiction: String,
    /// Markets withdrawn because their permission lapsed. Withdrawn for
    /// good: their books are dropped and nothing further is taken in, so a
    /// permission recorded again later lists a market only through a new
    /// installation, which asks every authority afresh.
    withdrawn: BTreeSet<ObjectId>,
}

impl EventMarketPlane {
    /// Open the plane over `adapter`, refusing a source whose licensing
    /// posture has not been evaluated.
    ///
    /// `creation` is the register every listing and quote is admitted
    /// against, in `jurisdiction`. There is no form of this that takes none:
    /// an empty register is the fail-closed one, under which the plane
    /// lists nothing and quotes nothing.
    #[allow(clippy::too_many_arguments)] // each is a separate authority the plane answers to
    pub fn open(
        adapter: Box<dyn PredictionAdapter + Send + Sync>,
        licensing: Option<LicensingDecision>,
        access: AccessBook,
        entity: impl Into<String>,
        oracle: OracleIdentity,
        creation: CreationPermissions,
        jurisdiction: impl Into<String>,
    ) -> Result<Self> {
        let source = adapter.descriptor();
        let posture = if source.is_synthetic {
            SourcePosture::Synthetic
        } else {
            match licensing {
                Some(decision)
                    if decision.source_id == source.name
                        && decision.usages.contains(&Usage::Research) =>
                {
                    SourcePosture::Admitted(Box::new(decision))
                }
                Some(decision) => {
                    return Err(Error::denied(format!(
                        "event source {} arrived with a licensing decision for {} permitting {:?}; \
                         a decision admits the source it names for the usages it lists, and \
                         this one does not admit {} for research",
                        source.name, decision.source_id, decision.usages, source.name
                    )));
                }
                None => {
                    return Err(Error::denied(format!(
                        "event source {} has no licensing evaluation, so it is refused before it \
                         is polled; admit it through qip-data-finder's licensing gate first",
                        source.name
                    )));
                }
            }
        };
        let entity = entity.into();
        if entity.trim().is_empty() {
            return Err(Error::invalid(
                "the event-market plane needs the entity its paper orders are admitted for",
            ));
        }
        let jurisdiction = jurisdiction.into();
        if jurisdiction.trim().is_empty() {
            return Err(Error::invalid(
                "the event-market plane needs the jurisdiction its markets were created in, \
                 which every listing and quote is admitted against; name it",
            ));
        }
        Ok(Self {
            adapter,
            source,
            posture,
            access,
            entity,
            oracle,
            definitions: DefinitionStore::new(),
            markets: BTreeMap::new(),
            books: BTreeMap::new(),
            resolutions: BTreeMap::new(),
            positions: BTreeMap::new(),
            cash: Decimal::ZERO,
            settled: BTreeSet::new(),
            traded_at: BTreeMap::new(),
            creation,
            jurisdiction,
            withdrawn: BTreeSet::new(),
        })
    }

    /// Replace the creation register, as an operator recording or revoking a
    /// permission does. The next pass withdraws every market it no longer
    /// admits, and quotes only what it admits for `seed`.
    pub fn set_creation_permissions(&mut self, creation: CreationPermissions) {
        self.creation = creation;
    }

    pub fn creation_permissions(&self) -> &CreationPermissions {
        &self.creation
    }

    /// Whether `market` was withdrawn because its permission lapsed.
    pub fn is_withdrawn(&self, market: &ObjectId) -> bool {
        self.withdrawn.contains(market)
    }

    /// Every refusal the register gives `action` on `market` and each of its
    /// outcome instruments, at the market's venue in this plane's
    /// jurisdiction. Empty when all are admitted.
    fn refusals(&self, action: CreationAction, market: &EventMarket) -> Vec<String> {
        std::iter::once(market.market_id.as_str())
            .chain(
                market
                    .outcomes()
                    .iter()
                    .map(|outcome| outcome.object_id.as_str()),
            )
            .filter_map(|product| {
                self.creation
                    .admit(&CreationRequest::new(
                        action,
                        market.venue.as_str(),
                        product,
                        self.jurisdiction.as_str(),
                    ))
                    .err()
                    .map(|error| format!("{product}: {}", error.message()))
            })
            .collect()
    }

    /// Withdraw every tracked market the register no longer admits for
    /// `create` or `list` — the two acts already taken on a market the plane
    /// holds — dropping its books so nothing is priced, traded or quoted on
    /// it. Returns one line per market withdrawn.
    fn withdraw_lapsed(&mut self) -> Vec<String> {
        let mut withdrawn = Vec::new();
        let live: Vec<ObjectId> = self
            .markets
            .keys()
            .filter(|id| !self.withdrawn.contains(*id))
            .cloned()
            .collect();
        for market_id in live {
            let Some(market) = self.markets.get(&market_id) else {
                continue;
            };
            let mut refused = self.refusals(CreationAction::Create, market);
            refused.extend(self.refusals(CreationAction::List, market));
            if refused.is_empty() {
                continue;
            }
            self.books.remove(&market_id);
            self.traded_at.remove(&market_id);
            self.withdrawn.insert(market_id.clone());
            withdrawn.push(format!(
                "{market_id} withdrawn, its creation permission lapsed: {}",
                refused.join("; ")
            ));
        }
        withdrawn
    }

    pub fn source(&self) -> &PredictionSource {
        &self.source
    }

    pub fn posture(&self) -> &SourcePosture {
        &self.posture
    }

    pub fn definitions(&self) -> &DefinitionStore {
        &self.definitions
    }

    pub fn markets(&self) -> impl Iterator<Item = &EventMarket> {
        self.markets.values()
    }

    pub fn resolution(&self, market: &ObjectId) -> Option<&MarketResolution> {
        self.resolutions.get(market)
    }

    pub fn position(&self, market: &ObjectId, outcome: &OutcomeId) -> Decimal {
        self.positions
            .get(&(market.clone(), outcome.clone()))
            .copied()
            .unwrap_or(Decimal::ZERO)
    }

    pub const fn cash(&self) -> Decimal {
        self.cash
    }

    /// Replace the access book, as an operator recording or revoking a
    /// permission does. Pricing is unaffected; orders are judged against it
    /// from the next pass.
    pub fn set_access(&mut self, access: AccessBook) {
        self.access = access;
    }

    fn decision_grade(&self) -> bool {
        self.posture.class().allows_production_decisions()
    }

    /// DISCOVER: ingest what the source published up to `now`.
    pub fn discover(&mut self, now: Timestamp) -> Result<EventMarketsDiscovered> {
        let mut found = EventMarketsDiscovered {
            source: self.source.name.clone(),
            posture: self.posture.as_str().to_string(),
            decision_grade: self.decision_grade(),
            ..EventMarketsDiscovered::default()
        };
        found.withdrawn = self.withdraw_lapsed();
        for update in self.adapter.poll(now)? {
            match update {
                PredictionUpdate::MarketListed(market) => {
                    let market = *market;
                    // A listing is taken in only under `list` for the market
                    // and every outcome instrument it trades as.
                    let refused = self.refusals(CreationAction::List, &market);
                    if !refused.is_empty() {
                        found.problems.push(format!(
                            "market {} was listed by the source and not taken in: the creation \
                             register does not admit listing it; {}",
                            market.market_id,
                            refused.join("; ")
                        ));
                        continue;
                    }
                    if !self.markets.contains_key(&market.market_id)
                        && self.markets.len() >= MAX_EVENT_MARKETS
                    {
                        found.problems.push(format!(
                            "market {} was listed past the bound of {MAX_EVENT_MARKETS} and was \
                             not tracked",
                            market.market_id
                        ));
                        continue;
                    }
                    let outcomes: BTreeSet<OutcomeId> = market
                        .outcomes()
                        .iter()
                        .map(|outcome| outcome.id.clone())
                        .collect();
                    // Valid and knowable from the instant it was polled: the
                    // listing is the first the platform knew of it, and a
                    // definition stamped earlier would be readable before it
                    // was knowable.
                    self.definitions.record(EventDefinition::new(
                        market.market_id.as_str(),
                        outcomes,
                        market.proposition.statement.clone(),
                        market.proposition.source.clone(),
                        now,
                        now,
                    )?)?;
                    self.resolutions
                        .entry(market.market_id.clone())
                        .or_insert_with(|| MarketResolution::new(&market, self.oracle.clone()));
                    found.listed.push(market.market_id.clone());
                    self.markets.insert(market.market_id.clone(), market);
                }
                PredictionUpdate::Book {
                    market_id,
                    outcome,
                    book,
                } => {
                    if self.withdrawn.contains(&market_id) {
                        continue;
                    }
                    let Some(market) = self.markets.get(&market_id) else {
                        found.problems.push(format!(
                            "a book for {market_id} arrived before its listing and was dropped"
                        ));
                        continue;
                    };
                    if market.outcome(&outcome).is_err() {
                        found.problems.push(format!(
                            "a book for outcome {outcome} names no outcome of {market_id}"
                        ));
                        continue;
                    }
                    found.books += 1;
                    self.books
                        .entry(market_id)
                        .or_default()
                        .insert(outcome, *book);
                }
                PredictionUpdate::Report { market_id, report } => {
                    match self.resolutions.get_mut(&market_id) {
                        Some(resolution) => {
                            if let Err(error) = resolution.observe(Some(report), now) {
                                found.problems.push(format!(
                                    "the oracle's report on {market_id} was refused: {}",
                                    error.message()
                                ));
                            }
                        }
                        None => found.problems.push(format!(
                            "a report on {market_id} arrived for a market this plane does not \
                             track"
                        )),
                    }
                }
            }
        }
        for (market_id, resolution) in &mut self.resolutions {
            if matches!(resolution.state(), ResolutionState::Proposed { .. }) {
                // Final only once the dispute window has closed; inside it
                // the refusal is the expected answer and not a problem.
                let _ = resolution.finalise(now);
            } else if let Err(error) = resolution.observe(None, now) {
                found.problems.push(format!(
                    "the resolution of {market_id} could not advance: {}",
                    error.message()
                ));
            }
            found.resolutions.insert(
                market_id.as_str().to_string(),
                resolution.state().as_str().to_string(),
            );
        }
        for (market_id, market) in &self.markets {
            if self.withdrawn.contains(market_id) {
                continue;
            }
            let Some(books) = self.complete_books(market) else {
                continue;
            };
            match set_arbitrage(market, &books) {
                Ok(Some(arbitrage)) => found.opportunities.push(EventOpportunity {
                    market_id: market_id.clone(),
                    kind: arbitrage.kind,
                    quantity: arbitrage.quantity,
                    profit: arbitrage.profit(),
                }),
                Ok(None) => {}
                Err(error) => found.problems.push(format!(
                    "the set arbitrage on {market_id} could not be sized: {}",
                    error.message()
                )),
            }
        }
        Ok(found)
    }

    /// The belief and fair value for one market, from its books alone.
    ///
    /// Takes no access book on purpose (5089): what an event is worth does
    /// not depend on who may trade it.
    pub fn price(
        market: &EventMarket,
        books: &[(OutcomeId, &OrderBook)],
        now: Timestamp,
    ) -> Result<(OutcomeDistribution, Valuation)> {
        let mut weights: BTreeMap<OutcomeId, Decimal> = BTreeMap::new();
        for (outcome, book) in books {
            let band = implied_band(book, &market.fees, market.payoff())?;
            weights.insert(outcome.clone(), band.midpoint()?.value());
        }
        let belief = normalise(weights, now)?;
        let valuation = fair_values(market, &belief)?;
        Ok((belief, valuation))
    }

    /// SIMULATE: price every market, then take what the access book admits,
    /// on paper.
    pub fn simulate(&mut self, now: Timestamp) -> Result<EventMarketsSimulated> {
        let mut run = EventMarketsSimulated {
            decision_grade: self.decision_grade(),
            ..EventMarketsSimulated::default()
        };
        run.withdrawn = self.withdraw_lapsed();
        let ids: Vec<ObjectId> = self
            .markets
            .keys()
            .filter(|id| !self.withdrawn.contains(*id))
            .cloned()
            .collect();
        for market_id in ids {
            let Some(market) = self.markets.get(&market_id).cloned() else {
                continue;
            };
            let Some(owned) = self.books.get(&market_id).cloned() else {
                continue;
            };
            let Some(books) = complete(&market, &owned) else {
                continue;
            };
            let valuation = match Self::price(&market, &books, now) {
                Ok((_, valuation)) => valuation,
                Err(error) => {
                    run.problems.push(format!(
                        "{market_id} could not be priced: {}",
                        error.message()
                    ));
                    continue;
                }
            };
            // The arbitrage, taken whole or not at all: a complete set with a
            // leg refused is not a hedge, it is a position. Taken once per
            // set of books: a pass that ran again before the venue published
            // would otherwise fill the same depth twice.
            let books_at = books
                .iter()
                .map(|(_, book)| book.at)
                .max()
                .unwrap_or(Timestamp::EPOCH);
            let fresh = self
                .traded_at
                .get(&market_id)
                .is_none_or(|last| books_at > *last);
            match set_arbitrage(&market, &books) {
                Ok(Some(arbitrage)) if fresh => {
                    let refused: Vec<String> = market
                        .outcomes()
                        .iter()
                        .filter_map(|outcome| {
                            self.access
                                .admit_event_order(
                                    &self.entity,
                                    &market.venue,
                                    outcome.object_id.as_str(),
                                )
                                .err()
                                .map(|error| error.message().to_string())
                        })
                        .collect();
                    if refused.is_empty() {
                        let sign = match arbitrage.kind {
                            SetArbitrageKind::UnderpricedSet => Decimal::ONE,
                            SetArbitrageKind::OverpricedSet => -Decimal::ONE,
                        };
                        for (outcome, price) in &arbitrage.leg_prices {
                            let quantity = sign * arbitrage.quantity;
                            *self
                                .positions
                                .entry((market_id.clone(), outcome.clone()))
                                .or_insert(Decimal::ZERO) += quantity;
                            run.fills.push(PaperEventFill {
                                market_id: market_id.clone(),
                                outcome: outcome.clone(),
                                quantity,
                                price: *price,
                            });
                        }
                        match arbitrage.kind {
                            SetArbitrageKind::UnderpricedSet => self.cash -= arbitrage.cost,
                            SetArbitrageKind::OverpricedSet => self.cash += arbitrage.proceeds,
                        }
                        self.traded_at.insert(market_id.clone(), books_at);
                    } else {
                        run.refusals.extend(
                            refused
                                .into_iter()
                                .map(|reason| format!("set arbitrage on {market_id}: {reason}")),
                        );
                    }
                }
                Ok(_) => {}
                Err(error) => run.problems.push(format!(
                    "the set arbitrage on {market_id} could not be sized: {}",
                    error.message()
                )),
            }
            // Paper quotes around fair value, each admitted by the same gate,
            // and each under `seed` for its instrument: a resting quote puts
            // liquidity into the market.
            for outcome in market.outcomes() {
                let Some(fair) = valuation.fair_values.get(&outcome.id).copied() else {
                    continue;
                };
                if let Err(error) = self.creation.admit(&CreationRequest::new(
                    CreationAction::Seed,
                    market.venue.as_str(),
                    outcome.object_id.as_str(),
                    self.jurisdiction.as_str(),
                )) {
                    run.refusals.push(format!(
                        "quote on {market_id}/{}: {}",
                        outcome.id,
                        error.message()
                    ));
                    continue;
                }
                if let Err(error) = self.access.admit_event_order(
                    &self.entity,
                    &market.venue,
                    outcome.object_id.as_str(),
                ) {
                    run.refusals.push(format!(
                        "quote on {market_id}/{}: {}",
                        outcome.id,
                        error.message()
                    ));
                    continue;
                }
                let bid = fair - PAPER_QUOTE_HALF_SPREAD;
                let ask = fair + PAPER_QUOTE_HALF_SPREAD;
                if bid.is_positive() && ask < market.payoff() {
                    run.quotes.push(PaperEventQuote {
                        market_id: market_id.clone(),
                        outcome: outcome.id.clone(),
                        bid,
                        ask,
                    });
                }
            }
            run.valuations.push(valuation);
        }
        run.settled = self.settle_final()?;
        Ok(run)
    }

    /// Settle every paper position on a market whose resolution is final,
    /// once. A market that is proposed, disputed or overdue is not touched:
    /// resolved is not settled.
    fn settle_final(&mut self) -> Result<Decimal> {
        let mut total = Decimal::ZERO;
        let finals: Vec<ObjectId> = self
            .resolutions
            .iter()
            .filter(|(id, resolution)| {
                resolution.state().is_settleable() && !self.settled.contains(*id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for market_id in finals {
            let Some(market) = self.markets.get(&market_id) else {
                continue;
            };
            let Some(resolution) = self.resolutions.get(&market_id) else {
                continue;
            };
            for ((held_market, outcome), quantity) in &self.positions {
                if held_market != &market_id || quantity.is_zero() {
                    continue;
                }
                let settlement = resolution.settle(outcome, quantity.abs(), &market.fees)?;
                if quantity.is_positive() {
                    total += settlement.net;
                } else {
                    // A short pays the gross payoff it sold and keeps no fee.
                    total -= settlement.gross;
                }
            }
            self.settled.insert(market_id);
        }
        self.cash += total;
        Ok(total)
    }

    fn complete_books<'a>(
        &'a self,
        market: &EventMarket,
    ) -> Option<Vec<(OutcomeId, &'a OrderBook)>> {
        complete(market, self.books.get(&market.market_id)?)
    }
}

/// Every outcome's book, or `None` while any outcome has none.
fn complete<'a>(
    market: &EventMarket,
    books: &'a BTreeMap<OutcomeId, OrderBook>,
) -> Option<Vec<(OutcomeId, &'a OrderBook)>> {
    market
        .outcomes()
        .iter()
        .map(|outcome| {
            books
                .get(&outcome.id)
                .map(|book| (outcome.id.clone(), book))
        })
        .collect()
}

/// Normalise implied midpoints into a distribution summing to exactly one,
/// the rounding residue on the most probable outcome (ties to the lowest
/// id) — the same rule `qip-prediction` applies to its own weights.
fn normalise(weights: BTreeMap<OutcomeId, Decimal>, now: Timestamp) -> Result<OutcomeDistribution> {
    let total: Decimal = weights.values().copied().sum();
    if !total.is_positive() {
        return Err(Error::invalid(
            "every outcome's implied probability is zero, so no belief can be formed",
        ));
    }
    let mut shares = BTreeMap::new();
    for (id, weight) in weights {
        let share = weight
            .checked_div(total)
            .ok_or_else(|| Error::numeric("an implied weight could not be normalised"))?;
        shares.insert(id, share);
    }
    let assigned: Decimal = shares.values().copied().sum();
    let top = shares
        .iter()
        .fold(
            None::<(OutcomeId, Decimal)>,
            |best, (id, share)| match best {
                Some((_, held)) if *share <= held => best,
                _ => Some((id.clone(), *share)),
            },
        )
        .map(|(id, _)| id)
        .ok_or_else(|| Error::invalid("no outcome to normalise"))?;
    if let Some(share) = shares.get_mut(&top) {
        *share += Decimal::ONE - assigned;
    }
    let mut probabilities = BTreeMap::new();
    for (id, share) in shares {
        probabilities.insert(id, Probability::new(share)?);
    }
    OutcomeDistribution::new(
        probabilities,
        now,
        "implied midpoints of the venue's touch, net of fees, normalised",
    )
}
