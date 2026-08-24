//! `qip-market-ingestion` — the SENSE stage.
//!
//! Everything the platform knows about the outside world enters here. Sources
//! sit behind narrow adapter traits so a provider can be replaced without
//! touching anything downstream, and every record is validated before it is
//! published: a record that fails validation becomes a
//! [`qip_financial::intelligence::DataQualityFailure`] rather than an
//! investment input.
//!
//! No licensed feed is subscribed to in this build. What ships instead is a
//! synthetic environment that is realistic in the ways that matter — stochastic
//! volatility, jumps, regime switching, a factor correlation structure, a
//! U-shaped intraday volume profile, spread that widens with volatility — plus
//! a replay adapter for recorded data. Both are marked
//! [`qip_financial::quality::LicensingClass::Synthetic`], which the object model
//! refuses to admit to a production decision. What a real deployment must
//! supply is listed in `docs/operations/external-dependencies.md`.
//!
//! Two adapters here are not generated. [`rest::RestMarketDataAdapter`] makes a
//! real HTTP request to a market-data vendor and decodes prices, bars, quotes
//! and trades; [`narrative::NarrativeAdapter`] makes one to a document vendor
//! and decodes news items, corporate filings and macroeconomic releases. Both
//! ship unconfigured, which they say through
//! [`adapter::SourceDescriptor::production_requirement`] and enforce by
//! refusing to poll — neither has a fallback to synthetic data, because a
//! stand-in that returns plausible prices, or a plausible headline, is
//! indistinguishable downstream from a feed that works.
//!
//! The document adapter carries two disciplines the price one does not need. A
//! document has two instants — filed and covering, published and describing —
//! and it is the one it *entered the world* at that decides when the platform
//! could act on it. And text carries redistribution terms that prices do not,
//! so every record leaves with a [`qip_financial::quality::LicensingClass`] and
//! a document that states none is refused rather than admitted under a default
//! that would permit its raw text to be displayed.

pub mod adapter;
pub mod narrative;
pub mod replay;
pub mod rest;
pub mod service;
pub mod synthetic;

pub use adapter::{
    AlternativeDataAdapter, DataAdapter, FundamentalsAdapter, MacroAdapter, MarketDataAdapter,
    NewsAdapter, SensedRecord, SourceDescriptor,
};
pub use narrative::{
    DocumentStats, NarrativeAdapter, NarrativeFeedConfig, NarrativeSubject, Revision,
};
pub use replay::ReplayAdapter;
pub use rest::{FetchStats, RestFeedConfig, RestInstrument, RestMarketDataAdapter};
pub use service::{IngestionService, IngestionStats};
pub use synthetic::{ScriptedEvent, ScriptedEventKind, SyntheticEnvironment, SyntheticInstrument};
