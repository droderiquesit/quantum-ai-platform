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
    /// What each held position is held *for*: its horizon, its hedges and
    /// the causal driver and factors it is allocated to (R12-ASSET-3019,
    /// ADR 0468). Written by the kernel from its position register beside
    /// the book itself; a default (empty) purpose states nothing, and the
    /// Portfolio/Asset Brain reports every position it does not cover as
    /// undeclared rather than as unhedged or open-ended.
    pub purpose: BookPurpose,
}

/// The investment horizon a held position was sized on.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldHorizon {
    /// The hypothesis the position was last sized on — the causal claim it
    /// is held for, and so the driver it is allocated to (ADR 0468).
    pub hypothesis: String,
    /// That hypothesis's own horizon.
    pub horizon: qip_core::Duration,
    /// When the sizing that recorded the horizon ran, where the register
    /// knows. `None` for a horizon resumed from the log with no sizing
    /// instant beside it: its age is then unknown, and the brain counts it
    /// as such rather than as within or past its horizon.
    pub sized_at: Option<qip_core::Timestamp>,
}

/// A held position's hedge standing as the register states it. A position
/// with no entry is *undeclared* — nobody looked — which is kept apart
/// from [`HedgeStanding::Unhedged`], a position the platform looked at and
/// found nothing covering.
#[derive(Clone, Debug, PartialEq)]
pub enum HedgeStanding {
    /// The declared hedge policies whose hedge is actually held.
    Hedged(Vec<String>),
    /// No hedge applies, with the reason the register gave.
    Unhedged { reason: String },
}

/// The purpose half of the book (R12-ASSET-3019, ADR 0468).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BookPurpose {
    /// Horizon by object id; a held position absent here has none on record.
    pub horizons: BTreeMap<String, HeldHorizon>,
    /// Hedge standing by object id; a held position absent here is
    /// undeclared.
    pub hedges: BTreeMap<String, HedgeStanding>,
    /// Why the register could not state any hedge this cycle, when it
    /// refused; every position is then undeclared and the brain says why.
    pub hedges_refused: Option<String>,
    /// Signed exposure per factor, in money, from the catalogue loadings
    /// `Portfolio::exposures` reads. Empty where no held instrument loads
    /// on any factor.
    pub factor_exposure: BTreeMap<String, qip_core::Decimal>,
    /// Why the factor breakdown could not be computed, when it refused.
    pub factors_refused: Option<String>,
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

/// One automated-market-maker pool as the confirmed chain holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct PoolReading {
    pub pool: String,
    pub reserve_base: qip_core::Decimal,
    pub reserve_quote: qip_core::Decimal,
    /// Swaps through the pool since it was first seen.
    pub trades: u64,
}

/// The chain as the kernel last confirmed it (R12-ASSET-3017): protocol
/// state at the confirmation depth, reorganisations and the bridge
/// transfers they failed, pool liquidity and on-chain flow. `observed` is
/// false until the kernel has absorbed any block, which the Crypto/On-chain
/// Brain reports as no data rather than as a quiet chain.
#[derive(Debug, Default)]
pub struct ChainView {
    pub observed: bool,
    /// The deepest block the confirmed view includes, when the chain is that
    /// deep.
    pub confirmed_block: Option<u64>,
    /// Reorganisations absorbed since the process started, and the deepest.
    pub reorgs: usize,
    pub deepest_reorg: u32,
    /// Bridge transfers failed because a reorganisation withdrew their
    /// deposit.
    pub bridged_transfers_failed: usize,
    /// Swaps counted in the confirmed view.
    pub confirmed_trades: Option<u64>,
    pub pools: Vec<PoolReading>,
}

/// The company records the Company/Fundamental Brain reads beside the
/// world model's reported fundamentals (R12-REASON-3013, ADR 0458): the
/// corporate actions the kernel holds awaiting their ex-date, and management
/// guidance and capital structures by issuer.
///
/// `corporate_actions_observed` is false until the kernel has been handed any
/// corporate action at all, so an empty list on a platform no corporate-action
/// feed reaches is reported as no source rather than as a quiet calendar.
/// Guidance and capital structure have no ingestion arm in the tree, so on
/// every deployed binary today both lists are empty and the brain names them
/// missing.
#[derive(Debug, Default)]
pub struct CompanyView {
    pub corporate_actions_observed: bool,
    /// Announced actions awaiting their ex-date, as the kernel holds them.
    pub corporate_actions: Vec<qip_market::corporate_action::CorporateAction>,
    pub guidance: Vec<qip_financial::company::ManagementGuidance>,
    pub capital_structures: Vec<qip_financial::company::CapitalStructure>,
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
    /// The chain as the kernel last confirmed it (R12-ASSET-3017). Empty
    /// from [`Desk::new`]; the kernel re-points this gate at the slot it
    /// feeds every REASON, and a test hands one in through
    /// [`Desk::with_chain`].
    pub chain: Gated<ChainView>,
    /// Corporate actions, guidance and capital structure (R12-REASON-3013).
    /// Empty from [`Desk::new`]; the kernel re-points this gate at the slot
    /// it feeds every REASON, and a test hands one in through
    /// [`Desk::with_company`].
    pub company: Gated<CompanyView>,
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
            // On-chain state is market data: blocks, pools and flows the
            // platform observes, with nothing of its own in them.
            chain: Gated::new(
                ChainView::default(),
                Capability::ReadMarketData,
                "chain-state",
            ),
            // Corporate actions, guidance and balance sheets describe
            // instruments and issuers: reference data, nothing of the book.
            company: Gated::new(
                CompanyView::default(),
                Capability::ReadReferenceData,
                "company-reference",
            ),
        }
    }

    /// The desk with `company` behind the company-reference gate, under the
    /// capability and label [`Desk::new`] chose for it.
    pub fn with_company(mut self, company: CompanyView) -> Self {
        self.company = Gated::new(company, self.company.required(), self.company.label());
        self
    }

    /// The desk with `chain` behind the chain-state gate, under the
    /// capability and label [`Desk::new`] chose for it.
    pub fn with_chain(mut self, chain: ChainView) -> Self {
        self.chain = Gated::new(chain, self.chain.required(), self.chain.label());
        self
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
                purpose: BookPurpose::default(),
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
