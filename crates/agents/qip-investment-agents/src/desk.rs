//! The facilities the organisation shares.
//!
//! Every agent holds an [`Arc<Desk>`] and reaches nothing except through the
//! [`Gated`] wrappers on it. A wrapper's inner value is private and its
//! accessor takes the run context, so an agent reading the book has
//! necessarily passed the `read_portfolio` check, been charged for the call,
//! and left an audit entry — whether or not its own code remembered to ask.
//!
//! One desk is shared by all eighteen agents. The capability grants differ per
//! agent, so the same desk presents a different surface to each: the macro
//! analyst holding no `read_portfolio` grant simply cannot see the book.

use qip_agents::capability::Capability;
use qip_agents::memory::ResearchMemory;
use qip_agents::runtime::Gated;
use qip_ai::retrieval::SearchIndex;
use qip_financial::constraints::RegulatoryConstraints;
use qip_financial::universe::Universe;
use qip_market::snapshot::MarketSnapshot;
use qip_portfolio::portfolio::Portfolio;
use qip_risk::limits::{LimitSet, RiskState};
use qip_world_model::WorldModel;
use std::collections::BTreeMap;

/// Live market state and the reference data that describes it.
#[derive(Debug)]
pub struct MarketView {
    pub snapshot: MarketSnapshot,
    pub universe: Universe,
}

/// The book, and the prices it is valued at.
#[derive(Debug)]
pub struct BookView {
    pub portfolio: Portfolio,
    /// Marks used for valuation, keyed by object id.
    pub marks: BTreeMap<String, qip_core::Decimal>,
}

/// The risk picture and the limits that bound it.
#[derive(Debug)]
pub struct RiskView {
    pub state: RiskState,
    pub limits: LimitSet,
}

/// Trading restrictions by object id.
///
/// Deterministic data rather than a model's reading of a policy document:
/// a compliance decision that depends on how a sentence was phrased is not a
/// compliance decision.
#[derive(Debug, Default)]
pub struct ComplianceView {
    pub constraints: BTreeMap<String, RegulatoryConstraints>,
    /// Names under an absolute prohibition, whatever else permits them.
    pub prohibited: Vec<String>,
}

impl ComplianceView {
    pub fn constraints_for(&self, object_id: &str) -> Option<&RegulatoryConstraints> {
        self.constraints.get(object_id)
    }

    pub fn is_prohibited(&self, object_id: &str) -> bool {
        self.prohibited.iter().any(|p| p == object_id)
    }
}

/// Everything the organisation can reach, each behind its capability.
#[derive(Debug)]
pub struct Desk {
    pub market: Gated<MarketView>,
    pub world: Gated<WorldModel>,
    pub book: Gated<BookView>,
    pub risk: Gated<RiskView>,
    pub compliance: Gated<ComplianceView>,
    pub memory: Gated<ResearchMemory>,
    pub library: Gated<SearchIndex>,
}

impl Desk {
    /// Wrap the facilities, each with the capability it requires.
    ///
    /// The pairing of facility to capability lives here and nowhere else, so
    /// there is one place to review when asking what a given grant unlocks.
    pub fn new(
        market: MarketView,
        world: WorldModel,
        book: BookView,
        risk: RiskView,
        compliance: ComplianceView,
        memory: ResearchMemory,
        library: SearchIndex,
    ) -> Self {
        Self {
            market: Gated::new(market, Capability::ReadMarketData, "market"),
            world: Gated::new(world, Capability::ReadWorldModel, "world-model"),
            book: Gated::new(book, Capability::ReadPortfolio, "portfolio"),
            risk: Gated::new(risk, Capability::ReadRiskState, "risk-state"),
            compliance: Gated::new(
                compliance,
                Capability::ReadReferenceData,
                "compliance-reference",
            ),
            memory: Gated::new(memory, Capability::ReadResearchMemory, "research-memory"),
            library: Gated::new(library, Capability::ReadNews, "research-library"),
        }
    }

    /// A desk with nothing in it, for tests and for a cold start.
    ///
    /// Agents run against it produce honest no-view findings rather than
    /// failing, which is the behaviour a platform starting with no data should
    /// have.
    pub fn empty(as_of: qip_core::Timestamp) -> Self {
        Self::new(
            MarketView {
                snapshot: MarketSnapshot::new(as_of),
                universe: Universe::new(),
            },
            WorldModel::new(),
            BookView {
                portfolio: Portfolio::new(
                    qip_core::PortfolioId::from_string("pf-empty"),
                    "empty",
                    qip_core::Currency::USD,
                    qip_core::Decimal::ZERO,
                    as_of,
                ),
                marks: BTreeMap::new(),
            },
            RiskView {
                state: RiskState::default(),
                limits: LimitSet::conservative_default(),
            },
            ComplianceView::default(),
            ResearchMemory::new(),
            SearchIndex::new(),
        )
    }
}
