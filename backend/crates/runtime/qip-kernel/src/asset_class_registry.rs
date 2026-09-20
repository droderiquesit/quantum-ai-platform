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
//! Nine of `AssetClass`'s thirteen variants, and the line is the blueprint's
//! own rather than this lane's judgement: §16.1's engine table has an
//! "Unlocks" column naming the classes each of the six engines prices, and
//! the nine registered here are exactly the classes that column names.
//! Read it with
//! `awk '/^16\.1 /{f=1} f&&/^16\.2 /{exit} f' docs/architecture/algorik-blueprint-v10.1-source.md`.
//!
//! The four it does not name — `ForeignExchange`, `Commodity`,
//! `DigitalAsset` and `Cash` — are **deliberately absent**, and that is a
//! finding rather than an omission: §17.7 says the six engines "cover every
//! pricing paradigm", and §16.1's own table does not reach four of the
//! platform's own thirteen classes. A record invented for them would have to
//! name one of the six engines, and the honest answer for a continuously
//! quoted spot instrument is that its mark is the quote — a paradigm the six
//! do not enumerate. Registering them on a guess is exactly the thing §17.7
//! exists to stop: a list that makes a capability claim nobody checked.
//!
//! **This is the list to change when a class becomes supported**, and the
//! change is a reviewed one: adding a record is adding a class the platform
//! says it can trade.
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

    #[test]
    fn the_shipped_registry_holds_nine_classes_and_names_the_four_it_refuses() -> Result<()> {
        // The premise first: `AssetClass` really does have thirteen variants,
        // so "nine registered" is a statement about coverage and not about a
        // list that happens to be the whole enum.
        assert_eq!(AssetClass::ALL.len(), 13);
        let registry = AssetClassRegistry::shipped()?;
        assert_eq!(registry.len(), 9);
        // Named rather than counted: a count would pass if the four swapped
        // for four others, and which four are unsupported is the finding.
        assert_eq!(
            registry.unregistered(),
            vec![
                AssetClass::ForeignExchange,
                AssetClass::Commodity,
                AssetClass::DigitalAsset,
                AssetClass::Cash,
            ]
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
}
