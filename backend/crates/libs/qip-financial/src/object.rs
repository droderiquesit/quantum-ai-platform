//! The financial object: one abstraction covering every instrument the platform
//! can hold.

use qip_core::error::{Error, Result};
use qip_core::{Currency, Decimal, ObjectId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::asset_class::{AssetClass, InstrumentType, Sector};
use crate::constraints::RegulatoryConstraints;
use crate::costs::{LiquidityProfile, TransactionCostModel};
use crate::extensions::Extension;
use crate::identifiers::Identifiers;
use crate::quality::{DataQuality, Provenance};
use crate::risk_profile::RiskCharacteristics;

/// A tradable or holdable financial instrument.
///
/// The base carries what every instrument has: identity, classification, where
/// and in what currency it trades, what it costs to trade, how liquid it is,
/// what risk it carries, and where the data came from. Instrument-specific
/// fields live in [`Self::extension`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinancialObject {
    pub object_id: ObjectId,
    pub symbol: String,
    pub name: String,
    pub asset_class: AssetClass,
    pub instrument_type: InstrumentType,
    pub identifiers: Identifiers,

    /// Legal issuer or obligor, where one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// The object this instrument derives from, for derivatives.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub underlying_object_id: Option<ObjectId>,
    /// Primary listing venue, or the OTC market convention.
    pub venue: String,
    /// ISO country code of primary risk.
    pub geography: String,
    pub sector: Sector,

    /// Currency the instrument is quoted in.
    pub currency: Currency,
    /// Currency it settles in — not always the quote currency.
    pub settlement_currency: Currency,

    /// Last known price in [`Self::currency`], per unit.
    pub price: Decimal,
    /// Yield as a decimal, where the instrument has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yield_rate: Option<f64>,
    /// Multiplier from one contract to the underlying notional.
    pub contract_multiplier: Decimal,
    /// Smallest tradable increment of quantity.
    pub lot_size: Decimal,
    /// Smallest price increment.
    pub tick_size: Decimal,

    pub liquidity: LiquidityProfile,
    pub transaction_costs: TransactionCostModel,
    pub regulatory: RegulatoryConstraints,
    pub risk: RiskCharacteristics,

    /// Instrument-specific detail.
    pub extension: Extension,

    /// Where this record came from and when.
    pub provenance: Provenance,
    pub quality: DataQuality,
    /// When the record was last updated in the platform.
    pub updated_at: Timestamp,
    /// Free-form provider attributes not modelled elsewhere.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub metadata: std::collections::BTreeMap<String, String>,
}

impl FinancialObject {
    /// Start building an object. Every required field must be supplied before
    /// [`ObjectBuilder::build`] will produce one.
    pub fn builder(
        object_id: ObjectId,
        symbol: impl Into<String>,
        instrument_type: InstrumentType,
    ) -> ObjectBuilder {
        ObjectBuilder::new(object_id, symbol, instrument_type)
    }

    /// Notional value of `quantity` units, in the quote currency.
    pub fn notional(&self, quantity: Decimal) -> Decimal {
        quantity * self.price * self.contract_multiplier
    }

    /// Round a quantity down to a whole number of lots.
    pub fn round_to_lot(&self, quantity: Decimal) -> Decimal {
        quantity.floor_to_step(self.lot_size)
    }

    /// Round a price to the tick grid.
    pub fn round_to_tick(&self, price: Decimal) -> Decimal {
        price.floor_to_step(self.tick_size)
    }

    /// Contractual maturity, from the extension.
    pub fn maturity(&self) -> Option<Timestamp> {
        self.extension.maturity()
    }

    /// Whether the instrument has matured as of `now`.
    pub fn is_expired(&self, now: Timestamp) -> bool {
        self.maturity().is_some_and(|m| m <= now)
    }

    /// Whether this object may currently be traded in the given direction.
    ///
    /// Deterministic: the compliance agent calls this rather than reasoning
    /// about restrictions in prose.
    pub fn tradability(&self, side: TradeSide) -> Tradability {
        let blockers = match side {
            TradeSide::Buy => self.regulatory.buy_blockers(),
            TradeSide::Sell => Vec::new(),
            TradeSide::SellShort => self.regulatory.short_blockers(),
        };
        if !blockers.is_empty() {
            return Tradability::Blocked {
                reasons: blockers.iter().map(|r| r.describe()).collect(),
            };
        }
        // A sell of an existing position is only blocked by restrictions that
        // trap a position outright.
        if side == TradeSide::Sell {
            let trapping = self.regulatory.liquidation_blockers();
            if !trapping.is_empty() {
                return Tradability::Blocked {
                    reasons: trapping.iter().map(|r| r.describe()).collect(),
                };
            }
        }
        Tradability::Allowed
    }

    /// Whether the record is fit to drive a capital decision.
    pub fn is_decision_grade(&self) -> bool {
        self.quality.meets(crate::quality::DECISION_QUALITY_FLOOR)
            && self.provenance.licensing.allows_production_decisions()
            && self.price.is_positive()
            && self.risk.is_coherent()
    }

    /// Structural problems with the record. Empty means internally consistent.
    ///
    /// This is the ingestion gate: an object failing any of these never reaches
    /// the world model.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if self.symbol.trim().is_empty() {
            issues.push("symbol is empty".into());
        }
        if self.asset_class != self.instrument_type.asset_class() {
            issues.push(format!(
                "asset class {} does not match instrument type {} (expected {})",
                self.asset_class,
                self.instrument_type,
                self.instrument_type.asset_class()
            ));
        }
        if self.instrument_type.is_derivative() && self.underlying_object_id.is_none() {
            issues.push(format!("{} requires an underlying", self.instrument_type));
        }
        if self.instrument_type.has_maturity() && self.extension.maturity().is_none() {
            issues.push(format!("{} requires a maturity", self.instrument_type));
        }
        if self.price.is_negative() {
            issues.push(format!("price {} is negative", self.price));
        }
        if !self.lot_size.is_positive() {
            issues.push("lot size must be positive".into());
        }
        if !self.tick_size.is_positive() {
            issues.push("tick size must be positive".into());
        }
        if !self.contract_multiplier.is_positive() {
            issues.push("contract multiplier must be positive".into());
        }
        if !self.risk.is_coherent() {
            issues.push("risk characteristics are incoherent".into());
        }
        if self.provenance.ingestion_time < self.provenance.event_time {
            issues.push("record was ingested before it happened".into());
        }
        for (kind, value) in self.identifiers.malformed() {
            issues.push(format!("malformed {kind}: {value}"));
        }
        issues
    }
}

/// Direction of a prospective trade, for the compliance check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TradeSide {
    Buy,
    Sell,
    SellShort,
}

/// Outcome of a tradability check.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Tradability {
    Allowed,
    Blocked { reasons: Vec<String> },
}

impl Tradability {
    pub fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    pub fn reasons(&self) -> &[String] {
        match self {
            Self::Allowed => &[],
            Self::Blocked { reasons } => reasons,
        }
    }
}

/// Builder that refuses to produce a structurally invalid object.
#[derive(Debug)]
pub struct ObjectBuilder {
    object_id: ObjectId,
    symbol: String,
    name: Option<String>,
    instrument_type: InstrumentType,
    identifiers: Identifiers,
    issuer: Option<String>,
    underlying_object_id: Option<ObjectId>,
    venue: String,
    geography: String,
    sector: Sector,
    currency: Currency,
    settlement_currency: Option<Currency>,
    price: Decimal,
    yield_rate: Option<f64>,
    contract_multiplier: Decimal,
    lot_size: Decimal,
    tick_size: Decimal,
    liquidity: LiquidityProfile,
    transaction_costs: TransactionCostModel,
    regulatory: RegulatoryConstraints,
    risk: RiskCharacteristics,
    extension: Extension,
    provenance: Option<Provenance>,
    quality: DataQuality,
    metadata: std::collections::BTreeMap<String, String>,
}

impl ObjectBuilder {
    pub fn new(
        object_id: ObjectId,
        symbol: impl Into<String>,
        instrument_type: InstrumentType,
    ) -> Self {
        Self {
            object_id,
            symbol: symbol.into(),
            name: None,
            instrument_type,
            identifiers: Identifiers::new(),
            issuer: None,
            underlying_object_id: None,
            venue: String::new(),
            geography: "US".into(),
            sector: Sector::Unclassified,
            currency: Currency::USD,
            settlement_currency: None,
            price: Decimal::ZERO,
            yield_rate: None,
            contract_multiplier: Decimal::ONE,
            lot_size: Decimal::ONE,
            tick_size: Decimal::from_raw(10_000_000), // 0.01
            liquidity: LiquidityProfile::default(),
            transaction_costs: TransactionCostModel::default(),
            regulatory: RegulatoryConstraints::unrestricted(),
            risk: RiskCharacteristics::default(),
            extension: Extension::Unspecified,
            provenance: None,
            quality: DataQuality::clean(),
            metadata: std::collections::BTreeMap::new(),
        }
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }
    pub fn identifiers(mut self, identifiers: Identifiers) -> Self {
        self.identifiers = identifiers;
        self
    }
    pub fn issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = Some(issuer.into());
        self
    }
    pub fn underlying(mut self, id: ObjectId) -> Self {
        self.underlying_object_id = Some(id);
        self
    }
    pub fn venue(mut self, venue: impl Into<String>) -> Self {
        self.venue = venue.into();
        self
    }
    pub fn geography(mut self, geography: impl Into<String>) -> Self {
        self.geography = geography.into();
        self
    }
    pub fn sector(mut self, sector: Sector) -> Self {
        self.sector = sector;
        self
    }
    pub fn currency(mut self, currency: Currency) -> Self {
        self.currency = currency;
        self
    }
    pub fn settlement_currency(mut self, currency: Currency) -> Self {
        self.settlement_currency = Some(currency);
        self
    }
    pub fn price(mut self, price: Decimal) -> Self {
        self.price = price;
        self
    }
    pub fn yield_rate(mut self, rate: f64) -> Self {
        self.yield_rate = Some(rate);
        self
    }
    pub fn contract_multiplier(mut self, multiplier: Decimal) -> Self {
        self.contract_multiplier = multiplier;
        self
    }
    pub fn lot_size(mut self, lot: Decimal) -> Self {
        self.lot_size = lot;
        self
    }
    pub fn tick_size(mut self, tick: Decimal) -> Self {
        self.tick_size = tick;
        self
    }
    pub fn liquidity(mut self, liquidity: LiquidityProfile) -> Self {
        self.liquidity = liquidity;
        self
    }
    pub fn transaction_costs(mut self, costs: TransactionCostModel) -> Self {
        self.transaction_costs = costs;
        self
    }
    pub fn regulatory(mut self, regulatory: RegulatoryConstraints) -> Self {
        self.regulatory = regulatory;
        self
    }
    pub fn risk(mut self, risk: RiskCharacteristics) -> Self {
        self.risk = risk;
        self
    }
    pub fn extension(mut self, extension: Extension) -> Self {
        self.extension = extension;
        self
    }
    pub fn provenance(mut self, provenance: Provenance) -> Self {
        self.provenance = Some(provenance);
        self
    }
    pub fn quality(mut self, quality: DataQuality) -> Self {
        self.quality = quality;
        self
    }
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Produce the object, rejecting a structurally invalid one.
    pub fn build(self, now: Timestamp) -> Result<FinancialObject> {
        let provenance = self
            .provenance
            .ok_or_else(|| Error::invalid("financial object requires provenance"))?;
        let symbol = self.symbol.clone();
        let object = FinancialObject {
            object_id: self.object_id,
            name: self.name.unwrap_or_else(|| symbol.clone()),
            symbol,
            asset_class: self.instrument_type.asset_class(),
            instrument_type: self.instrument_type,
            identifiers: self.identifiers,
            issuer: self.issuer,
            underlying_object_id: self.underlying_object_id,
            venue: self.venue,
            geography: self.geography,
            sector: self.sector,
            settlement_currency: self.settlement_currency.unwrap_or(self.currency),
            currency: self.currency,
            price: self.price,
            yield_rate: self.yield_rate,
            contract_multiplier: self.contract_multiplier,
            lot_size: self.lot_size,
            tick_size: self.tick_size,
            liquidity: self.liquidity,
            transaction_costs: self.transaction_costs,
            regulatory: self.regulatory,
            risk: self.risk,
            extension: self.extension,
            provenance,
            quality: self.quality,
            updated_at: now,
            metadata: self.metadata,
        };
        let issues = object.validate();
        if !issues.is_empty() {
            return Err(Error::invalid(format!(
                "invalid financial object {}: {}",
                object.symbol,
                issues.join("; ")
            )));
        }
        Ok(object)
    }
}
