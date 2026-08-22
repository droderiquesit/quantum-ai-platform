//! `qip-market-ingestion` — the SENSE stage.
//!
//! Everything the platform knows about the outside world enters here. Sources
//! sit behind narrow adapter traits so a provider can be replaced without
//! touching anything downstream, and every record is validated before it is
//! published: a record that fails validation becomes a
//! [`qip_financial::intelligence::DataQualityFailure`] rather than an
//! investment input.
//!
//! No licensed feed is available to this build. What ships instead is a
//! synthetic environment that is realistic in the ways that matter — stochastic
//! volatility, jumps, regime switching, a factor correlation structure, a
//! U-shaped intraday volume profile, spread that widens with volatility — plus
//! a replay adapter for recorded data. Both are marked
//! [`qip_financial::quality::LicensingClass::Synthetic`], which the object model
//! refuses to admit to a production decision. What a real deployment must
//! supply is listed in `docs/operations/external-dependencies.md`.

pub mod adapter;
pub mod replay;
pub mod service;
pub mod synthetic;

pub use adapter::{
    AlternativeDataAdapter, DataAdapter, FundamentalsAdapter, MacroAdapter, MarketDataAdapter,
    NewsAdapter, SensedRecord, SourceDescriptor,
};
pub use replay::ReplayAdapter;
pub use service::{IngestionService, IngestionStats};
pub use synthetic::{ScriptedEvent, ScriptedEventKind, SyntheticEnvironment, SyntheticInstrument};
