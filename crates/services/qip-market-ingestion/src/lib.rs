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
//!
//! Two more adapters open sockets, and each closes the last gap in one class of
//! input. [`depth::DepthFeedAdapter`] builds real order books — it fetches a
//! depth snapshot, applies the vendor's increments through `qip_sequencing`'s
//! gap detection, and feeds `qip_orderbook`, which had never seen a book this
//! platform did not write itself. It is the first adapter here that holds
//! *state* rather than decoding records, so its whole discipline is about a
//! book being wrong rather than a record being wrong: a sequence gap forces a
//! rebuild instead of a silent apply, a diverged book is withheld rather than
//! published, and a crossed book is either the auction `qip_orderbook::auction`
//! models or corruption to rebuild from — never something to normalise into a
//! plausible shape.
//!
//! [`alternative::AlternativeFeedAdapter`] decodes satellite, IoT, mobility and
//! web readings. It carries the document adapter's two disciplines and sharpens
//! both: a reading has *three* instants — captured, processed, published — and
//! only the last is one a consumer could have acted at; and because alternative
//! data is the most restrictively licensed input a fund buys, a reading with no
//! licensing class is refused. It adds a third: a vendor that filled a gap must
//! say so, since [`qip_financial::quality::DataQuality`]'s default asserts a
//! perfectly measured value and an unstated quality would arrive as one.

pub mod adapter;
pub mod alternative;
pub mod depth;
pub mod narrative;
pub mod replay;
pub mod rest;
pub mod service;
pub mod synthetic;

pub use adapter::{
    AlternativeDataAdapter, DataAdapter, FundamentalsAdapter, MacroAdapter, MarketDataAdapter,
    NewsAdapter, SensedRecord, SourceDescriptor,
};
pub use alternative::{
    AlternativeFeedAdapter, AlternativeFeedConfig, AlternativeStats, AlternativeSubject,
};
pub use depth::{DepthFeedAdapter, DepthFeedConfig, DepthInstrument, DepthStats};
pub use narrative::{
    DocumentStats, NarrativeAdapter, NarrativeFeedConfig, NarrativeSubject, Revision,
};
pub use replay::ReplayAdapter;
pub use rest::{FetchStats, RestFeedConfig, RestInstrument, RestMarketDataAdapter};
pub use service::{IngestionService, IngestionStats};
pub use synthetic::{ScriptedEvent, ScriptedEventKind, SyntheticEnvironment, SyntheticInstrument};
