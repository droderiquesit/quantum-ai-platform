//! `qip-financial` — the universal financial object model.
//!
//! The platform is deliberately not built around equities. Every instrument it
//! can hold — a Treasury bill, a CLO mezzanine tranche, a listed option, a
//! private-equity commitment, a spot FX pair — is a [`FinancialObject`]: a
//! common base carrying identity, currency, venue, liquidity, cost and risk
//! characteristics, plus one typed [`Extension`] holding the fields that only
//! make sense for that instrument type.
//!
//! The alternative — one flat struct with every field nullable — puts the
//! burden of "does this field mean anything here?" on every caller. The
//! alternative in the other direction — an unrelated struct per asset class —
//! makes a cross-asset portfolio impossible to express. A common base plus
//! typed extensions is the workable middle, and it is what section 3 of the
//! platform charter asks for.

pub mod asset_class;
pub mod calendar;
pub mod catalogue;
pub mod constraints;
pub mod costs;
pub mod extensions;
pub mod identifiers;
pub mod intelligence;
pub mod object;
pub mod quality;
pub mod risk_profile;
pub mod universe;

pub use asset_class::{AssetClass, InstrumentType, Sector};
pub use calendar::{MarketHours, Session, TradingCalendar};
pub use catalogue::{CatalogueManifest, LoadedCatalogue};
pub use constraints::{Jurisdiction, RegulatoryConstraints, TradingRestriction};
pub use costs::{LiquidityProfile, TransactionCostModel};
pub use extensions::Extension;
pub use identifiers::{IdentifierKind, Identifiers};
pub use intelligence::{
    AlternativeDataPoint, EntityMention, FundamentalUpdate, MacroObservation, NewsItem, Sentiment,
};
pub use object::{FinancialObject, ObjectBuilder};
pub use quality::{DataQuality, LicensingClass, Provenance};
pub use risk_profile::{FactorExposures, Greeks, RiskCharacteristics};
pub use universe::Universe;
