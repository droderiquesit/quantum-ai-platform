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

/// Which way an execution went, as the desk tells its readers.
///
/// A local enum rather than the execution engine's `Side`, because this crate
/// may not depend on the execution engine (libs and agents sit beneath the
/// services): the kernel maps one onto the other where it feeds the desk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TradeSide {
    Buy,
    Sell,
}

impl TradeSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }

    /// `+1` for a buy, `-1` for a sell: the sign that turns a price
    /// difference into "worse for us".
    pub fn sign(self) -> f64 {
        match self {
            Self::Buy => 1.0,
            Self::Sell => -1.0,
        }
    }
}

/// One of the platform's own executed orders, as an adversary probe reads it
/// (R12-REASON-3023): what was asked, what was paid, and when.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionRecord {
    pub order_id: String,
    pub object_id: String,
    pub side: TradeSide,
    /// Quantity filled. Always positive.
    pub quantity: qip_core::Decimal,
    /// The reference price when the order was created, the base slippage
    /// is measured from.
    pub arrival_price: qip_core::Decimal,
    /// Quantity-weighted fill price.
    pub fill_price: qip_core::Decimal,
    pub placed_at: qip_core::Timestamp,
    pub filled_at: qip_core::Timestamp,
    pub venue: String,
}

impl ExecutionRecord {
    /// Slippage against arrival in basis points, positive when the fill was
    /// worse for the platform: paid more on a buy, received less on a sell.
    /// A statistic (`f64`) computed from money (`Decimal`); the crossing is
    /// here, once.
    pub fn adverse_slippage_bp(&self) -> Option<f64> {
        let arrival = self.arrival_price.to_f64();
        let fill = self.fill_price.to_f64();
        (arrival.is_finite() && arrival > 0.0 && fill.is_finite())
            .then(|| self.side.sign() * (fill - arrival) / arrival * 10_000.0)
    }
}

/// The platform's own order and fill history, bounded, most recent last.
///
/// What a probe for strategies that exploit the platform itself reads: an
/// adversary who has learned the platform's flow shows up as fills that are
/// systematically worse than arrival, and as a flow regular enough to be
/// learned. Empty on a desk the kernel has not fed, which the probe reports
/// as nothing examined rather than as nothing found.
#[derive(Debug, Default)]
pub struct ExecutionView {
    pub executions: Vec<ExecutionRecord>,
}

/// One forecaster's standing in the platform's tournament, as the
/// model-failure probe reads it (R12-REASON-3023).
#[derive(Clone, Debug, PartialEq)]
pub struct ForecasterHealth {
    pub name: String,
    /// Mean Brier score over the questions the tournament has scored it on;
    /// `None` until one has resolved.
    pub mean_brier: Option<f64>,
    /// The meta-model's predicted probability that the forecaster fails in
    /// the current regime, in `[0, 1]`.
    pub predicted_failure: f64,
}

/// The models the platform runs on, as the kernel knows them: every
/// forecaster the tournament holds a question from, and every production
/// model the deep brain last reported past its own drift threshold.
#[derive(Debug, Default)]
pub struct ModelsView {
    pub forecasters: Vec<ForecasterHealth>,
    /// `{name}@{version}` references past their drift threshold.
    pub drifted: Vec<String>,
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
    /// The platform's own executions (R12-REASON-3023). Empty from
    /// [`Desk::new`]; the kernel re-points this gate at the slot it feeds
    /// every REASON, and a test hands one in through
    /// [`Desk::with_executions`].
    pub executions: Gated<ExecutionView>,
    /// The platform's models as the kernel scores them (R12-REASON-3023).
    /// Empty from [`Desk::new`]; the kernel re-points this gate at the slot
    /// it feeds every REASON, and a test hands one in through
    /// [`Desk::with_models`].
    pub models: Gated<ModelsView>,
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
            executions: Gated::new(
                ExecutionView::default(),
                Capability::ReadExecutionHistory,
                "execution-history",
            ),
            models: Gated::new(
                ModelsView::default(),
                Capability::ReadModelRegistry,
                "model-registry",
            ),
        }
    }

    /// The desk with `models` behind the model-registry gate, under the
    /// capability and label [`Desk::new`] chose for it.
    pub fn with_models(mut self, models: ModelsView) -> Self {
        self.models = Gated::new(models, self.models.required(), self.models.label());
        self
    }

    /// The desk with `executions` behind the execution-history gate, under
    /// the capability and label [`Desk::new`] chose for it.
    pub fn with_executions(mut self, executions: ExecutionView) -> Self {
        self.executions = Gated::new(
            executions,
            self.executions.required(),
            self.executions.label(),
        );
        self
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
