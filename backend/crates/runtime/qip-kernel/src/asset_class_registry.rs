//! The asset class registry (blueprint §17.7): one record per class saying
//! how it is priced, how it settles, how it trades, and which strategies may
//! touch it — and the gate between *architecturally reachable* and *actually
//! supported*.
//!
//! §17.7's argument is that "hundreds of asset classes" is a capability claim
//! until there is a list, and that "a class with no valuation engine, no
//! settlement convention, or no eligible family cannot be registered, and an
//! unregistered class cannot be traded". Both halves are here: the record,
//! and [`Platform::new`](crate::Platform::new) refusing to assemble over a
//! universe holding an instrument in a class this registry does not name.
//!
//! # Why the refusal stops assembly rather than joining a list
//!
//! The kernel already collects `Universe::not_decision_grade` — instruments
//! refused on licensing, price, coherence or quality — and **nothing reads
//! it to refuse anything**: it is a field, a gauge and an accessor, the whole
//! universe moves into the desk regardless, and `is_decision_grade` is called
//! by no kernel path at all (`grep -rn 'is_decision_grade' backend/crates
//! --include=*.rs | grep -v /tests/`). Pushing an unregistered class onto that
//! list would have produced a record, not a control — the
//! `MaxExpectedShortfall` shape, protection that cannot fire.
//!
//! So the registry refuses where a composition root refuses everything else
//! it cannot accept: at assembly, before anything is served. A universe is
//! configuration — a committed catalogue with a digest — and an instrument
//! in a class the platform cannot price, settle or assign a family to is an
//! invalid configuration, not a runtime surprise. The process stops naming
//! the class and the three things §17.7 requires of one.
//!
//! # Which classes are registered, and why exactly those
//!
//! **All fifteen of `AssetClass`'s variants**, and the count is not written
//! down so that it can drift: recount with
//! `grep -c 'add(AssetClassRecord::new(' backend/crates/runtime/qip-kernel/src/asset_class_registry.rs`
//! and compare with `AssetClass::ALL.len()`. This paragraph said "nine of
//! thirteen" until 2026-10-11, by which time the enum had fifteen variants
//! and the table held twelve: `Prediction`, `Commodity` and
//! `PhysicalProduct` had been registered and the enum had grown two arms,
//! and the list of "four deliberately absent" classes still named
//! `Commodity`, which was by then registered. A module doc that miscounts
//! the table it documents is a capability claim nobody checked — the thing
//! §17.7 exists to stop — so the figure is gone and the command stays.
//!
//! The original nine follow §16.1's "Unlocks" column, read with
//! `awk '/^16\.1 /{f=1} f&&/^16\.2 /{exit} f' docs/architecture/algorik-blueprint-v10.1-source.md`.
//! The last three — `ForeignExchange`, `Cash` and `DigitalAsset`, registered
//! for R12-COVERAGE-5098 and -5103 — are the ones that column does not name,
//! and this module used to argue that a continuously quoted spot
//! instrument's mark *is* the quote, a paradigm the six engines do not
//! enumerate. That argument was about spot alone, and the requirement is
//! not spot alone: it is spot **and forwards, swaps, funding and
//! conversion** for cash and FX, and spot **and derivatives and
//! tokenized representations** for digital assets. Every one of those
//! beyond the spot mark is priced off a curve — covered interest parity
//! for an FX forward ([`covered_interest_forward`]), the short end of the
//! discount curve for a cash equivalent, and the basis and funding term
//! structure for a crypto future or perpetual — so the three register on
//! [`ValuationEngine::TermStructure`], with the spot mark as that curve's
//! zero-tenor point. No seventh paradigm was invented to do it, which is
//! the line the closed enum below draws.
//!
//! **This is the list to change when a class becomes supported**, and the
//! change is a reviewed one: adding a record is adding a class the platform
//! says it can trade. With every variant registered, the refusal at
//! assembly still fires for any class the table does not hold — proven
//! against a registry with one record withdrawn ([`AssetClassRegistry::without`]),
//! because the shipped table no longer leaves one out to prove it with.
//!
//! # What is held by the type system and what is checked
//!
//! Two of §17.7's three registration refusals are structural rather than
//! runtime: [`AssetClassRecord`] has no constructor that omits the valuation
//! engine or the settlement, because neither field is an `Option`, so a
//! record with no engine is not a state this module checks for — it is a
//! state that cannot be written. The third is a runtime refusal, because a
//! `BTreeSet` can be empty: a record naming no eligible family is refused by
//! [`AssetClassRecord::new`], with the sentence saying what to do instead.

use crate::platform::Platform;
use qip_capital::collateral::MarginRegime;
use qip_capital_fabric::settlement::SettlementConvention;
use qip_core::Decimal;
use qip_core::error::{Error, Result};
use qip_financial::asset_class::AssetClass;
use qip_financial::object::FinancialObject;
use qip_optimization_engine::universe::AlphaFamily;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Which of §16.1's six paradigms prices a class.
///
/// Closed, and closed at six on purpose: §16.1 names six engines and says
/// they cover every pricing paradigm. A seventh arm is a claim about how this
/// platform values things and belongs in an ADR, not in a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValuationEngine {
    /// Yield curves, discount factors, forwards, convexity, roll.
    TermStructure,
    /// Default probability, recovery, spread decomposition, covenant state.
    Credit,
    /// Surface with skew and term structure, dispersion, forward volatility.
    VolatilitySurface,
    /// A mark with a method and a confidence — `qip_financial::valuation`.
    IlliquidValuation,
    /// Irregular contingent streams, call schedules, drawdown and J-curve.
    CashflowAndCommitments,
    /// Splits, dividends, mergers, spinoffs, rights, delistings.
    CorporateActions,
}

impl ValuationEngine {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TermStructure => "term_structure",
            Self::Credit => "credit",
            Self::VolatilitySurface => "volatility_surface",
            Self::IlliquidValuation => "illiquid_valuation",
            Self::CashflowAndCommitments => "cashflow_and_commitments",
            Self::CorporateActions => "corporate_actions",
        }
    }
}

/// How a class settles.
///
/// Wraps `qip_capital_fabric::settlement::SettlementConvention` rather than
/// using it bare, because §17.7's own example column says "Crypto spot
/// instant; equity T+1; **private, quarterly statement**", and a quarterly
/// statement is not a T+n. Forcing one on a private fund would tell the
/// settlement projection that a position is deliverable in two days, which is
/// worse than saying nothing: it is a number nobody computed, in a control
/// that reads as knowledge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassSettlement {
    /// Delivery versus payment on the venue's cycle.
    Exchange(SettlementConvention),
    /// No delivery cycle: the position is marked and reconciled when the
    /// administrator reports. `qip_financial::cashflow` is where its
    /// obligations live.
    PeriodicStatement,
}

impl ClassSettlement {
    pub fn describe(self) -> String {
        match self {
            Self::Exchange(convention) => convention.as_str().to_string(),
            Self::PeriodicStatement => "periodic statement".to_string(),
        }
    }
}

/// What sizes a class can express.
///
/// The per-*instrument* grid already exists and already refuses — §18.1's
/// `VenueFeasibility`, installed on the order manager at assembly. This is
/// the per-*class* floor beneath it, and it answers a question the instrument
/// grid cannot: whether a record claiming a finer grid than the class permits
/// is reference data to be trusted. It is a floor and never a substitute.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GridRule {
    /// A quoted market: prices move in ticks and quantities in lots, and no
    /// instrument of this class may claim a finer tick than `minimum_tick`.
    Quoted {
        minimum_tick: Decimal,
        minimum_lot: Decimal,
    },
    /// Negotiated: size and price are what a counterparty agrees, so the
    /// class imposes no floor and this record refuses nothing on its account.
    Negotiated,
}

/// When a class is live.
///
/// Deliberately not `qip_financial::calendar::MarketHours`, which names a
/// venue and is keyed by venue in that module: a class does not trade at one
/// venue, and a class record carrying one venue's sessions would be a second,
/// class-shaped copy of a venue's fact — the kind of duplicate that is only
/// noticed when the two disagree. Each arm names the `MarketHours`
/// constructor that builds its shape, so the two stay tied without either
/// owning the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TradingCalendar {
    /// Never closes — `MarketHours::continuous`'s shape.
    Continuous,
    /// An exchange session with holidays — `MarketHours::weekday_session`'s.
    ExchangeSession,
    /// By appointment: no session, no calendar, a price when someone agrees.
    Negotiated,
}

/// Which jurisdiction rules apply, for the tax engine.
///
/// A closed set of treatments rather than a jurisdiction, because
/// `qip_financial::constraints::Jurisdiction` says *where* and this says
/// *how*: a corporate bond and a common share are both United States
/// instruments and are not taxed alike. The arms are the distinctions
/// `qip_capital::ledger::lot`'s holding-period machinery can actually act
/// on; a finer taxonomy would be arms nothing reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaxTreatment {
    /// Capital gains by holding period; dividends as income.
    MarketableSecurity,
    /// Interest accrues as income; discount and premium amortise.
    DebtInstrument,
    /// Flow-through: the vehicle reports, the holder is taxed on the report.
    Partnership,
    /// Collectible or real property: its own rate and its own holding rules.
    RealProperty,
}

/// One class's record — §17.7's nine fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssetClassRecord {
    class: AssetClass,
    valuation_engine: ValuationEngine,
    settlement: ClassSettlement,
    grid: GridRule,
    calendar: TradingCalendar,
    margin: MarginRegime,
    /// Whether `qip_market::corporate_action` applies. §16.1: "Equities yes;
    /// crypto spot no; tokenized RWA depends on the wrapper."
    corporate_actions_apply: bool,
    tax: TaxTreatment,
    /// Which of §19's ten alpha families may trade this class.
    eligible_families: BTreeSet<AlphaFamily>,
    /// What can offset it, as classes rather than instruments: a hedge map
    /// entry naming an object id would be a second instrument registry, and
    /// `qip_risk::hedge::HedgeInstrument` — which does name one — is declared
    /// per policy by a person, which is where an instrument-level hedge
    /// belongs.
    hedge_classes: BTreeSet<AssetClass>,
}

impl AssetClassRecord {
    /// Build a record, refusing the one of §17.7's three conditions the type
    /// system does not already hold.
    ///
    /// No valuation engine and no settlement convention are unrepresentable:
    /// both fields are required and neither is an `Option`. No eligible
    /// family is representable — an empty `BTreeSet` — and is refused here,
    /// because a class no strategy family may trade is a class the platform
    /// would carry reference data for and never act on, which is the
    /// capability claim §17.7 exists to refuse.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        class: AssetClass,
        valuation_engine: ValuationEngine,
        settlement: ClassSettlement,
        grid: GridRule,
        calendar: TradingCalendar,
        margin: MarginRegime,
        corporate_actions_apply: bool,
        tax: TaxTreatment,
        eligible_families: BTreeSet<AlphaFamily>,
        hedge_classes: BTreeSet<AssetClass>,
    ) -> Result<Self> {
        if eligible_families.is_empty() {
            return Err(Error::invalid(format!(
                "asset class {} names no eligible strategy family, so nothing could ever trade \
                 it; name the families of blueprint §19 that may, or leave the class \
                 unregistered — an unregistered class is refused honestly and a registered one \
                 no family may touch is reference data pretending to be support",
                class.as_str()
            )));
        }
        if let GridRule::Quoted {
            minimum_tick,
            minimum_lot,
        } = grid
        {
            if !minimum_tick.is_positive() {
                return Err(Error::invalid(format!(
                    "asset class {} states a minimum tick of {minimum_tick}, which is not a \
                     price increment; state the finest increment a venue in this class quotes, \
                     or declare the class negotiated",
                    class.as_str()
                )));
            }
            if !minimum_lot.is_positive() {
                return Err(Error::invalid(format!(
                    "asset class {} states a minimum lot of {minimum_lot}, which is not a \
                     quantity; state the smallest quantity this class trades in, or declare the \
                     class negotiated",
                    class.as_str()
                )));
            }
        }
        Ok(Self {
            class,
            valuation_engine,
            settlement,
            grid,
            calendar,
            margin,
            corporate_actions_apply,
            tax,
            eligible_families,
            hedge_classes,
        })
    }

    pub const fn class(&self) -> AssetClass {
        self.class
    }

    pub const fn valuation_engine(&self) -> ValuationEngine {
        self.valuation_engine
    }

    pub const fn settlement(&self) -> ClassSettlement {
        self.settlement
    }

    pub const fn grid(&self) -> GridRule {
        self.grid
    }

    pub const fn calendar(&self) -> TradingCalendar {
        self.calendar
    }

    pub const fn margin(&self) -> MarginRegime {
        self.margin
    }

    pub const fn corporate_actions_apply(&self) -> bool {
        self.corporate_actions_apply
    }

    pub const fn tax(&self) -> TaxTreatment {
        self.tax
    }

    pub const fn eligible_families(&self) -> &BTreeSet<AlphaFamily> {
        &self.eligible_families
    }

    pub const fn hedge_classes(&self) -> &BTreeSet<AssetClass> {
        &self.hedge_classes
    }

    /// Whether `family` may trade this class.
    pub fn admits_family(&self, family: AlphaFamily) -> bool {
        self.eligible_families.contains(&family)
    }

    /// Refuse an instrument whose quoted grid is finer than this class
    /// permits.
    ///
    /// One direction only, and it is the safe one: a record claiming a tick
    /// *finer* than the class trades in tells every downstream control that a
    /// price is expressible which no venue would accept, and §18.1's
    /// feasibility gate would then pass a size the market refuses. A coarser
    /// tick is a venue's own business and is left alone.
    pub fn admit_instrument(&self, object: &FinancialObject) -> Result<()> {
        let GridRule::Quoted { minimum_tick, .. } = self.grid else {
            return Ok(());
        };
        if object.tick_size < minimum_tick {
            return Err(Error::invalid(format!(
                "{} is a {} quoted in ticks of {}, finer than the {minimum_tick} this class \
                 expresses; correct the reference record — a price the platform can state and \
                 no venue can accept passes the feasibility gate and is refused at the book",
                object.object_id.as_str(),
                self.class.as_str(),
                object.tick_size
            )));
        }
        Ok(())
    }
}

/// Every class this platform says it supports.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetClassRegistry {
    records: BTreeMap<AssetClass, AssetClassRecord>,
}

impl AssetClassRegistry {
    /// The shipped table, reviewed like a constant.
    ///
    /// Every value is a governance decision recorded in source — the
    /// discipline `qip_risk::hedge::HedgePolicy` already keeps, and for the
    /// same reason: the place for judgement is in *setting* the table, and a
    /// registry that derived its own entries could not be audited. The
    /// valuation engine of each row is §16.1's "Unlocks" column; the
    /// settlement and corporate-action columns are §17.7's own example
    /// column where it gives one.
    pub fn shipped() -> Result<Self> {
        let listed_grid = GridRule::Quoted {
            // One part in ten thousand, in the units the price is quoted in.
            // Not a venue's tick — every venue in the committed catalogue
            // quotes coarser, and a venue's own grid is on the instrument —
            // but the level below which a quoted increment is a reference
            // data error rather than a market. The catalogue's six
            // instruments quote in hundredths and clear it by two orders of
            // magnitude, which is the margin a floor should have.
            minimum_tick: Decimal::from_raw(100_000),
            minimum_lot: Decimal::from_raw(100_000),
        };
        let mut records = BTreeMap::new();
        let mut add = |record: AssetClassRecord| {
            records.insert(record.class, record);
        };

        use AlphaFamily as F;

        // Equity — §16.1: corporate actions "Unlocks: Equities, without which
        // positions and cost basis silently corrupt". T+1 is §17.7's own
        // example. Every family but carry, which needs a funding leg a common
        // share does not have.
        add(AssetClassRecord::new(
            AssetClass::Equity,
            ValuationEngine::CorporateActions,
            ClassSettlement::Exchange(SettlementConvention::T1),
            listed_grid,
            TradingCalendar::ExchangeSession,
            MarginRegime::Isolated,
            true,
            TaxTreatment::MarketableSecurity,
            [
                F::MarketMaking,
                F::Arbitrage,
                F::Microstructure,
                F::ShortHorizonReversion,
                F::MomentumAndTrend,
                F::StatisticalArbitrage,
                F::EventDriven,
                F::Volatility,
                F::ExecutionAlpha,
            ]
            .into_iter()
            .collect(),
            [AssetClass::Equity, AssetClass::Derivative]
                .into_iter()
                .collect(),
        )?);

        // Fixed income — §16.1: term structure "Unlocks: Government and
        // corporate bonds". Carry is the family the funding leg exists for.
        add(AssetClassRecord::new(
            AssetClass::FixedIncome,
            ValuationEngine::TermStructure,
            ClassSettlement::Exchange(SettlementConvention::T1),
            listed_grid,
            TradingCalendar::ExchangeSession,
            MarginRegime::Isolated,
            true,
            TaxTreatment::DebtInstrument,
            [
                F::Carry,
                F::StatisticalArbitrage,
                F::MomentumAndTrend,
                F::EventDriven,
                F::Arbitrage,
                F::ExecutionAlpha,
            ]
            .into_iter()
            .collect(),
            [AssetClass::Rates, AssetClass::Credit]
                .into_iter()
                .collect(),
        )?);

        // Credit — §16.1: the credit engine "Unlocks: Corporate bonds, credit
        // default swaps, private credit, distressed". T+2, the convention a
        // cash bond settles on where an equity is T+1.
        add(AssetClassRecord::new(
            AssetClass::Credit,
            ValuationEngine::Credit,
            ClassSettlement::Exchange(SettlementConvention::T2),
            listed_grid,
            TradingCalendar::ExchangeSession,
            MarginRegime::Isolated,
            true,
            TaxTreatment::DebtInstrument,
            [
                F::Carry,
                F::StatisticalArbitrage,
                F::EventDriven,
                F::Arbitrage,
                F::ExecutionAlpha,
            ]
            .into_iter()
            .collect(),
            [AssetClass::Credit, AssetClass::FixedIncome]
                .into_iter()
                .collect(),
        )?);

        // Rates — §16.1: term structure "Unlocks: swaps, futures fair value".
        // Portfolio margin: a rates book is netted by the clearer, which is
        // the distinction `MarginRegime` exists to record.
        add(AssetClassRecord::new(
            AssetClass::Rates,
            ValuationEngine::TermStructure,
            ClassSettlement::Exchange(SettlementConvention::T0),
            listed_grid,
            TradingCalendar::ExchangeSession,
            MarginRegime::Portfolio,
            false,
            TaxTreatment::DebtInstrument,
            [
                F::Carry,
                F::StatisticalArbitrage,
                F::Arbitrage,
                F::MomentumAndTrend,
                F::ExecutionAlpha,
            ]
            .into_iter()
            .collect(),
            [AssetClass::Rates, AssetClass::FixedIncome]
                .into_iter()
                .collect(),
        )?);

        // Derivative — §16.1: the volatility surface "Unlocks: Options beyond
        // parity arbitrage, variance trading". Registered on the engine the
        // blueprint names, and a reader should know the engine is the one ADR
        // 0050 bars from being wired to synthetic inputs: the class is
        // supported as a record, and what may price it is bounded elsewhere.
        add(AssetClassRecord::new(
            AssetClass::Derivative,
            ValuationEngine::VolatilitySurface,
            ClassSettlement::Exchange(SettlementConvention::T0),
            listed_grid,
            TradingCalendar::ExchangeSession,
            MarginRegime::Portfolio,
            false,
            TaxTreatment::MarketableSecurity,
            [
                F::Volatility,
                F::Arbitrage,
                F::MarketMaking,
                F::StatisticalArbitrage,
                F::EventDriven,
                F::ExecutionAlpha,
            ]
            .into_iter()
            .collect(),
            [AssetClass::Derivative, AssetClass::Equity]
                .into_iter()
                .collect(),
        )?);

        // Prediction — event outcomes that settle to fixed payoffs at a
        // specified future date. Priced on volatility surface like derivatives,
        // instant settlement.
        add(AssetClassRecord::new(
            AssetClass::Prediction,
            ValuationEngine::VolatilitySurface,
            ClassSettlement::Exchange(SettlementConvention::T0),
            listed_grid,
            TradingCalendar::ExchangeSession,
            MarginRegime::Isolated,
            false,
            TaxTreatment::MarketableSecurity,
            [
                F::EventDriven,
                F::Arbitrage,
                F::StatisticalArbitrage,
                F::Volatility,
                F::ExecutionAlpha,
            ]
            .into_iter()
            .collect(),
            [AssetClass::Derivative].into_iter().collect(),
        )?);

        // Structured product — §16.1: the volatility surface "Unlocks:
        // structured payoffs".
        add(AssetClassRecord::new(
            AssetClass::StructuredProduct,
            ValuationEngine::VolatilitySurface,
            ClassSettlement::Exchange(SettlementConvention::T2),
            listed_grid,
            TradingCalendar::ExchangeSession,
            MarginRegime::Isolated,
            false,
            TaxTreatment::DebtInstrument,
            [F::Carry, F::Volatility, F::EventDriven]
                .into_iter()
                .collect(),
            [AssetClass::Derivative, AssetClass::FixedIncome]
                .into_iter()
                .collect(),
        )?);

        // Commodity — §16.1: term structure "Unlocks: Commodity futures,
        // swaps, and spot trading". Commodities are exchange-traded where
        // futures exist, and term-structure pricing is standard. They hedge
        // physical products and real assets.
        add(AssetClassRecord::new(
            AssetClass::Commodity,
            ValuationEngine::TermStructure,
            ClassSettlement::Exchange(SettlementConvention::T0),
            listed_grid,
            TradingCalendar::ExchangeSession,
            MarginRegime::Portfolio,
            false,
            TaxTreatment::MarketableSecurity,
            [F::Carry, F::Arbitrage, F::MomentumAndTrend]
                .into_iter()
                .collect(),
            [AssetClass::RealAsset, AssetClass::PhysicalProduct]
                .into_iter()
                .collect(),
        )?);

        // Fund — §16.1: cashflow and commitments "Unlocks: Private funds".
        // A fund reports; it does not settle on a venue cycle.
        add(AssetClassRecord::new(
            AssetClass::Fund,
            ValuationEngine::CashflowAndCommitments,
            ClassSettlement::PeriodicStatement,
            GridRule::Negotiated,
            TradingCalendar::Negotiated,
            MarginRegime::Isolated,
            false,
            TaxTreatment::Partnership,
            [F::Carry, F::MomentumAndTrend].into_iter().collect(),
            BTreeSet::new(),
        )?);

        // Private market — §16.1: illiquid valuation "Unlocks: Private equity
        // and venture". The class `private_holdings_of` marks at assembly.
        add(AssetClassRecord::new(
            AssetClass::PrivateMarket,
            ValuationEngine::IlliquidValuation,
            ClassSettlement::PeriodicStatement,
            GridRule::Negotiated,
            TradingCalendar::Negotiated,
            MarginRegime::Isolated,
            false,
            TaxTreatment::Partnership,
            [F::Carry].into_iter().collect(),
            BTreeSet::new(),
        )?);

        // Real asset — §16.1: illiquid valuation "Unlocks: real estate, art,
        // collectibles".
        add(AssetClassRecord::new(
            AssetClass::RealAsset,
            ValuationEngine::IlliquidValuation,
            ClassSettlement::PeriodicStatement,
            GridRule::Negotiated,
            TradingCalendar::Negotiated,
            MarginRegime::Isolated,
            false,
            TaxTreatment::RealProperty,
            [F::Carry].into_iter().collect(),
            BTreeSet::new(),
        )?);

        // Physical product — commerce plane (§20) "Unlocks: SKU-level commerce,
        // auctions, collectibles, wholesale/retail arbitrage, inventory". Uses
        // illiquid valuation, as physical goods cannot be priced continuously.
        // Settlement is periodic: purchase, logistics, resale and settlement
        // complete over time. Tax treatment as RealProperty for physical goods.
        add(AssetClassRecord::new(
            AssetClass::PhysicalProduct,
            ValuationEngine::IlliquidValuation,
            ClassSettlement::PeriodicStatement,
            GridRule::Negotiated,
            TradingCalendar::Negotiated,
            MarginRegime::Isolated,
            false,
            TaxTreatment::RealProperty,
            [F::Carry].into_iter().collect(),
            [AssetClass::Commodity].into_iter().collect(),
        )?);

        // Foreign exchange — R12-COVERAGE-5098: spot, forwards and swaps.
        // Term structure, because a forward is spot carried along two
        // discount curves (`covered_interest_forward`) and a swap is a spot
        // and a forward; the spot mark is the zero-tenor point. T+2 is the
        // spot convention in most pairs. The grid is FX's own rather than
        // `listed_grid`: a pair quotes in pipettes, a hundred-thousandth,
        // which the listed floor would refuse as a reference-data error.
        // Continuous because the market rolls from Sydney to New York with
        // no session a calendar could close; the weekend gap is a venue's
        // fact and lives on its `MarketHours`, not on the class.
        add(AssetClassRecord::new(
            AssetClass::ForeignExchange,
            ValuationEngine::TermStructure,
            ClassSettlement::Exchange(SettlementConvention::T2),
            GridRule::Quoted {
                // 0.00001 and 0.001 units.
                minimum_tick: Decimal::from_raw(10_000),
                minimum_lot: Decimal::from_raw(1_000_000),
            },
            TradingCalendar::Continuous,
            MarginRegime::Portfolio,
            false,
            TaxTreatment::MarketableSecurity,
            [
                F::Carry,
                F::Arbitrage,
                F::MarketMaking,
                F::Microstructure,
                F::MomentumAndTrend,
                F::StatisticalArbitrage,
                F::ExecutionAlpha,
            ]
            .into_iter()
            .collect(),
            [
                AssetClass::ForeignExchange,
                AssetClass::Cash,
                AssetClass::Rates,
            ]
            .into_iter()
            .collect(),
        )?);

        // Cash and cash equivalents — R12-COVERAGE-5098: fiat balances,
        // money-market instruments, funding. Priced on the short end of the
        // discount curve, which is a term structure at its shortest tenor.
        // Same-day settlement. Negotiated grid: a deposit or a sweep has no
        // book and no tick, and a floor here would refuse a balance of an
        // odd number of cents. Carry is the one family it serves — funding
        // and the yield on idle balances — and a desk that wanted to trade
        // cash any other way would be trading FX.
        add(AssetClassRecord::new(
            AssetClass::Cash,
            ValuationEngine::TermStructure,
            ClassSettlement::Exchange(SettlementConvention::T0),
            GridRule::Negotiated,
            TradingCalendar::ExchangeSession,
            MarginRegime::Isolated,
            false,
            TaxTreatment::DebtInstrument,
            [F::Carry, F::ExecutionAlpha].into_iter().collect(),
            [AssetClass::Rates, AssetClass::ForeignExchange]
                .into_iter()
                .collect(),
        )?);

        // Digital asset — R12-COVERAGE-5103: crypto spot, its derivatives and
        // tokenized representations. Term structure for the reason above:
        // futures basis and perpetual funding are the curve, spot its zero
        // tenor. Settlement is instant on-chain finality as far as the class
        // is concerned (§17.7's own example, "crypto spot instant"); the
        // stages a transfer passes through on the way — pending, confirmed,
        // final — are a property of the *path*, and `qip_arbitrage::graph`
        // carries them on its nodes. The grid is a satoshi, one
        // hundred-millionth, for tick and lot alike. Corporate actions do
        // not apply to the class (§16.1: "crypto spot no"); a tokenized
        // security's wrapper may carry them, and the instrument record is
        // where that is stated, not the class.
        add(AssetClassRecord::new(
            AssetClass::DigitalAsset,
            ValuationEngine::TermStructure,
            ClassSettlement::Exchange(SettlementConvention::T0),
            GridRule::Quoted {
                minimum_tick: Decimal::from_raw(10),
                minimum_lot: Decimal::from_raw(10),
            },
            TradingCalendar::Continuous,
            MarginRegime::Isolated,
            false,
            TaxTreatment::MarketableSecurity,
            [
                F::Arbitrage,
                F::MarketMaking,
                F::Microstructure,
                F::ShortHorizonReversion,
                F::MomentumAndTrend,
                F::StatisticalArbitrage,
                F::Carry,
                F::Volatility,
                F::ExecutionAlpha,
            ]
            .into_iter()
            .collect(),
            [AssetClass::DigitalAsset, AssetClass::Derivative]
                .into_iter()
                .collect(),
        )?);

        let registry = Self { records };
        registry.check_hedge_map()?;
        Ok(registry)
    }

    /// Refuse a table whose hedge map names a class the table does not hold.
    ///
    /// A hedge into an unregistered class is an offset the platform could
    /// never take, and a map that names one reads as coverage. Checked over
    /// the whole table rather than per record, because a record cannot see
    /// its siblings.
    fn check_hedge_map(&self) -> Result<()> {
        for record in self.records.values() {
            for hedge in &record.hedge_classes {
                if !self.records.contains_key(hedge) {
                    return Err(Error::invalid(format!(
                        "asset class {} is hedged with {}, which this registry does not hold; \
                         register {} or drop it from the hedge map — an offset in a class the \
                         platform may not trade is an offset it could never take",
                        record.class.as_str(),
                        hedge.as_str(),
                        hedge.as_str()
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, class: AssetClass) -> Option<&AssetClassRecord> {
        self.records.get(&class)
    }

    pub fn is_registered(&self, class: AssetClass) -> bool {
        self.records.contains_key(&class)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &AssetClassRecord> {
        self.records.values()
    }

    /// Every `AssetClass` variant this registry does not hold, in declaration
    /// order — the answer to "which classes is the platform architecturally
    /// reachable for and not actually supported in".
    pub fn unregistered(&self) -> Vec<AssetClass> {
        AssetClass::ALL
            .into_iter()
            .filter(|class| !self.is_registered(*class))
            .collect()
    }

    /// This registry with `class`'s record withdrawn.
    ///
    /// The narrowing direction only: there is no counterpart that adds a
    /// record from outside [`Self::shipped`], because adding a class is the
    /// reviewed change and withdrawing one is always safe. Refused when the
    /// withdrawal would leave a sibling hedged into the withdrawn class — the
    /// same refusal `shipped` applies to its own table — so a narrowed
    /// registry is never one `shipped` could not have produced.
    pub fn without(mut self, class: AssetClass) -> Result<Self> {
        self.records.remove(&class);
        self.check_hedge_map()?;
        Ok(self)
    }

    /// Admit one instrument, or refuse naming its class.
    ///
    /// §17.7's "an unregistered class cannot be traded", as the one sentence
    /// an operator reads when a catalogue reaches the platform carrying an
    /// instrument it may not act on.
    pub fn admit(&self, object: &FinancialObject) -> Result<()> {
        match self.records.get(&object.asset_class) {
            Some(record) => record.admit_instrument(object),
            None => Err(Error::invalid(format!(
                "{} is a {} and no record registers that class, so this platform may not trade \
                 it; a class is registered only with a valuation engine, a settlement \
                 convention and at least one eligible strategy family (blueprint §17.7). \
                 Remove the instrument from the catalogue, or register the class in \
                 `qip_kernel::asset_class_registry::AssetClassRegistry::shipped` — which is a \
                 reviewed change, because it is the platform saying it can price, settle and \
                 size this class",
                object.object_id.as_str(),
                object.asset_class.as_str()
            ))),
        }
    }
}

/// An FX forward outright by covered interest parity:
/// `spot × (1 + r_domestic × days / basis) / (1 + r_foreign × days / basis)`.
///
/// Why `ForeignExchange` registers on [`ValuationEngine::TermStructure`]: a
/// forward is not quoted independently of spot, it is spot carried along the
/// two currencies' discount curves, and a forward priced any other way is an
/// arbitrage against the two deposits. `spot` is units of the domestic (quote)
/// currency per unit of foreign (base); rates are simple money-market rates
/// as fractions — exact, because they are charged against money — and
/// `basis` is the day-count denominator (360 for most pairs, 365 for GBP and
/// a few others), which is a convention of the pair and is therefore the
/// caller's to state rather than this function's to assume.
///
/// Refused rather than clamped: a non-positive spot, a zero basis, or a
/// growth factor at or below zero (a rate so negative the deposit is
/// confiscated) is a caller error that a clamp would turn into a price.
pub fn covered_interest_forward(
    spot: Decimal,
    domestic_rate: Decimal,
    foreign_rate: Decimal,
    days: u32,
    basis: u32,
) -> Result<Decimal> {
    if !spot.is_positive() {
        return Err(Error::invalid(format!(
            "an FX spot of {spot} is not a price; a forward is carried from a positive spot"
        )));
    }
    if basis == 0 {
        return Err(Error::invalid(
            "a day-count basis of zero divides by nothing; state the pair's convention, 360 or 365",
        ));
    }
    let year_fraction = Decimal::from_int(i64::from(days))
        .checked_div(Decimal::from_int(i64::from(basis)))
        .ok_or_else(|| Error::numeric("the forward's year fraction overflowed"))?;
    let growth = |rate: Decimal, leg: &str| -> Result<Decimal> {
        let factor = rate
            .checked_mul(year_fraction)
            .and_then(|accrual| Decimal::ONE.checked_add(accrual))
            .ok_or_else(|| Error::numeric(format!("the {leg} accrual overflowed")))?;
        if !factor.is_positive() {
            return Err(Error::invalid(format!(
                "a {leg} rate of {rate} over {days} days on a {basis}-day basis leaves a growth \
                 factor of {factor}; a deposit cannot be worth nothing or less at maturity, so \
                 the rate is wrong"
            )));
        }
        Ok(factor)
    };
    let domestic = growth(domestic_rate, "domestic")?;
    let foreign = growth(foreign_rate, "foreign")?;
    spot.checked_mul(domestic)
        .and_then(|carried| carried.checked_div(foreign))
        .ok_or_else(|| Error::numeric("the forward outright overflowed"))
}

impl Platform {
    /// The classes this platform is registered to trade (§17.7).
    pub fn asset_class_registry(&self) -> &AssetClassRegistry {
        &self.asset_class_registry
    }
}

#[cfg(test)]
#[allow(clippy::panic_in_result_fn)]
mod tests {
    use super::*;

    /// R12-COVERAGE-5098 and -5103. Fails if any of the three new records is
    /// deleted from `shipped` (the count drops and `unregistered` names it),
    /// and the per-class assertions fail if a record is registered on a
    /// guess — the wrong settlement, or the listed grid that would refuse a
    /// pipette or a satoshi as a reference-data error.
    #[test]
    fn every_asset_class_is_registered_including_fx_cash_and_digital_assets() -> Result<()> {
        // Premise: the enum has fifteen arms, so "every one registered" is a
        // statement about coverage and not about a short list.
        assert_eq!(AssetClass::ALL.len(), 15);
        let registry = AssetClassRegistry::shipped()?;
        assert_eq!(registry.unregistered(), Vec::<AssetClass>::new());
        assert_eq!(registry.len(), AssetClass::ALL.len());

        let fx = registry
            .get(AssetClass::ForeignExchange)
            .ok_or_else(|| Error::invalid("FX is not registered"))?;
        assert_eq!(fx.valuation_engine(), ValuationEngine::TermStructure);
        assert_eq!(
            fx.settlement(),
            ClassSettlement::Exchange(SettlementConvention::T2)
        );
        assert!(fx.admits_family(AlphaFamily::Carry));
        assert_eq!(
            fx.grid(),
            GridRule::Quoted {
                minimum_tick: Decimal::from_raw(10_000),
                minimum_lot: Decimal::from_raw(1_000_000),
            }
        );

        let cash = registry
            .get(AssetClass::Cash)
            .ok_or_else(|| Error::invalid("cash is not registered"))?;
        assert_eq!(
            cash.settlement(),
            ClassSettlement::Exchange(SettlementConvention::T0)
        );
        assert_eq!(cash.grid(), GridRule::Negotiated);

        let digital = registry
            .get(AssetClass::DigitalAsset)
            .ok_or_else(|| Error::invalid("digital assets are not registered"))?;
        assert_eq!(
            digital.settlement(),
            ClassSettlement::Exchange(SettlementConvention::T0)
        );
        assert!(!digital.corporate_actions_apply());
        assert!(digital.admits_family(AlphaFamily::Arbitrage));
        assert_eq!(
            digital.grid(),
            GridRule::Quoted {
                minimum_tick: Decimal::from_raw(10),
                minimum_lot: Decimal::from_raw(10),
            }
        );
        Ok(())
    }

    /// The gate still fires once every class is registered: withdrawing a
    /// record makes that class's instrument refused by the same `admit` the
    /// platform calls at assembly. Fails if `without` stops removing the
    /// record or `admit` stops consulting the table.
    #[test]
    fn withdrawing_a_class_makes_its_instruments_refused_by_the_same_admission() -> Result<()> {
        use qip_financial::asset_class::{InstrumentType, Sector};
        use qip_financial::costs::LiquidityProfile;
        use qip_financial::quality::Provenance;
        let at = qip_core::Timestamp::from_secs(1_760_000_000);
        let coin = FinancialObject::builder(
            qip_core::ObjectId::from_string("obj-BTC"),
            "obj-BTC",
            InstrumentType::Cryptocurrency,
            LiquidityProfile::listed(Decimal::from_int(5_000_000), 3.0),
        )
        .venue("SIM")
        .sector(Sector::Financials)
        .price(Decimal::from_int(100))
        .provenance(Provenance::synthetic("asset-class-test", at))
        .build(at)?;
        let shipped = AssetClassRegistry::shipped()?;
        // Premise: the shipped table admits it.
        shipped.admit(&coin)?;
        let narrowed = shipped.without(AssetClass::DigitalAsset)?;
        let refused = narrowed
            .admit(&coin)
            .expect_err("a withdrawn class's instrument was admitted");
        assert!(
            refused
                .message()
                .contains("obj-BTC is a digital_asset and no record registers"),
            "{}",
            refused.message()
        );
        // Withdrawing a class a sibling hedges into is refused, as `shipped`
        // would refuse such a table.
        assert!(
            AssetClassRegistry::shipped()?
                .without(AssetClass::Rates)
                .is_err()
        );
        Ok(())
    }

    /// Covered interest parity against a hand computation: 1.10 spot, 5%
    /// domestic, 3% foreign, 90 days on 360 is 1.10 × 1.0125 / 1.0075. Fails
    /// if either growth factor is dropped or the two are swapped.
    #[test]
    fn an_fx_forward_is_spot_carried_along_both_curves() -> Result<()> {
        let d = |text: &str| Decimal::parse(text).ok_or_else(|| Error::invalid("literal"));
        let forward = covered_interest_forward(d("1.10")?, d("0.05")?, d("0.03")?, 90, 360)?;
        let expected = d("1.11375")?
            .checked_div(d("1.0075")?)
            .ok_or_else(|| Error::invalid("division"))?;
        assert_eq!(forward, expected);
        // Premise of the comparison: the forward differs from spot, so the
        // equality is not met by a function that returns spot.
        assert!(forward > d("1.10")?);
        assert!(
            covered_interest_forward(Decimal::ZERO, Decimal::ZERO, Decimal::ZERO, 90, 360).is_err()
        );
        assert!(
            covered_interest_forward(Decimal::ONE, Decimal::ZERO, Decimal::ZERO, 90, 0).is_err()
        );
        Ok(())
    }

    #[test]
    fn a_record_naming_no_eligible_family_is_refused_and_one_with_no_engine_cannot_be_written()
    -> Result<()> {
        // §17.7's third registration condition, the only one of the three a
        // runtime check has to hold: the other two are unrepresentable,
        // which this test states by construction — `AssetClassRecord::new`
        // has no call below that omits the engine or the settlement, because
        // there is no such call to write.
        let refused = AssetClassRecord::new(
            AssetClass::Cash,
            ValuationEngine::TermStructure,
            ClassSettlement::Exchange(SettlementConvention::T0),
            GridRule::Negotiated,
            TradingCalendar::Continuous,
            MarginRegime::Isolated,
            false,
            TaxTreatment::MarketableSecurity,
            BTreeSet::new(),
            BTreeSet::new(),
        )
        .expect_err("a class no family may trade was registered");
        assert!(
            refused.message().contains("no eligible strategy family"),
            "{}",
            refused.message()
        );
        // And the same record with one family stands, so the refusal is
        // about the family set and not about the rest of the row.
        assert!(
            AssetClassRecord::new(
                AssetClass::Cash,
                ValuationEngine::TermStructure,
                ClassSettlement::Exchange(SettlementConvention::T0),
                GridRule::Negotiated,
                TradingCalendar::Continuous,
                MarginRegime::Isolated,
                false,
                TaxTreatment::MarketableSecurity,
                [AlphaFamily::Carry].into_iter().collect(),
                BTreeSet::new(),
            )
            .is_ok()
        );
        Ok(())
    }

    #[test]
    fn every_hedge_class_the_shipped_table_names_is_itself_registered() -> Result<()> {
        let registry = AssetClassRegistry::shipped()?;
        // Premise: the table names hedges at all, so the check below is not
        // walking an empty set.
        let named: usize = registry
            .iter()
            .map(|record| record.hedge_classes().len())
            .sum();
        assert!(named >= 8, "only {named} hedge class(es) are named");
        for record in registry.iter() {
            for hedge in record.hedge_classes() {
                assert!(
                    registry.is_registered(*hedge),
                    "{} is hedged with the unregistered {}",
                    record.class().as_str(),
                    hedge.as_str()
                );
            }
        }
        Ok(())
    }

    #[test]
    fn prediction_is_registered_as_a_distinct_class_from_derivative() -> Result<()> {
        let registry = AssetClassRegistry::shipped()?;
        // Prediction must be in the registry.
        let prediction_record = registry
            .get(AssetClass::Prediction)
            .expect("Prediction is not registered");
        // Derivative must also be in the registry.
        let derivative_record = registry
            .get(AssetClass::Derivative)
            .expect("Derivative is not registered");
        // They must be distinct records.
        assert_ne!(
            prediction_record, derivative_record,
            "Prediction and Derivative must be distinct registry entries"
        );
        // Prediction must use VolatilitySurface like Derivative.
        assert_eq!(
            prediction_record.valuation_engine(),
            ValuationEngine::VolatilitySurface,
            "Prediction must use VolatilitySurface valuation engine"
        );
        // But must settle T+0 (instant) unlike Derivative's T+0 (which is the
        // same, but confirm the settlement convention is set correctly).
        match prediction_record.settlement() {
            ClassSettlement::Exchange(SettlementConvention::T0) => {}
            _ => panic!("Prediction must settle on T0 (instant)"),
        }
        // Prediction must admit EventDriven family.
        assert!(
            prediction_record.admits_family(AlphaFamily::EventDriven),
            "Prediction must admit EventDriven family"
        );
        Ok(())
    }
}
