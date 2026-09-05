//! Four worked connectors: two against endpoints that need no key and no
//! signup, and the two candidates ADR 0034 names for equities and
//! prediction markets.
//!
//! They are here rather than in a test file because an example nobody compiles
//! is an example that stops being true. All four are ordinary
//! [`crate::connector::SourceConnector`] implementations, all four ship their
//! manifest and a fixture, and all four are run through
//! [`crate::connector::ContractHarness`] in `tests/connector_contract.rs` with
//! no network.
//!
//! | connector | source | what it produces |
//! |---|---|---|
//! | [`coinbase_ticker`] | `api.exchange.coinbase.com/products/BTC-USD/ticker` | a [`qip_market::quote::Tick`] per last trade |
//! | [`frankfurter_rates`] | `api.frankfurter.dev/v1/latest?base=EUR` | a [`qip_financial::intelligence::MacroObservation`] per currency pair |
//! | [`kalshi_markets`] | `api.elections.kalshi.com/trade-api/v2/markets` | a [`qip_market::quote::Quote`] per open binary market |
//! | [`alpaca_bars`] | `data.alpaca.markets/v2/stocks/bars` | a [`qip_market::bar::Bar`] per symbol per session |
//!
//! # All four are unreachable in this build, and say so
//!
//! `qip_transport::http` has no TLS stack and refuses `https` by name rather
//! than downgrading it. Every one of these endpoints is HTTPS only. So every
//! manifest ships with **no `base_url`**, which makes
//! [`crate::connector::manifest::SourceManifest::missing_configuration`] name
//! what is missing and
//! [`crate::connector::transport::HttpSourceTransport::connect`] refuse.
//!
//! A deployment supplies the address of a TLS-terminating egress proxy in
//! front of the source. That is the same requirement
//! `RestMarketDataAdapter::REQUIREMENTS` states, and it is stated here rather
//! than worked around because a connector that fell back to plaintext would
//! send requests — and, for a source that needed one, a credential — across
//! the internet in clear text.
//!
//! # Two of the four are refused by the licensing gate today
//!
//! Kalshi and Alpaca are candidates whose terms have not been read against a
//! contract (ADR 0034). Their manifests declare the fail-closed licensing
//! floor, `qip-data-finder`'s catalogue carries an `Ambiguous` posture for
//! each naming the terms to read, and `admission::admit` refuses both. Their
//! hosts are in neither the egress allowlist nor the Envoy bootstrap; that is
//! ADR 0034's separate step, once the terms are read. The connectors exist so
//! that step is a licensing decision and an allowlist entry, not a rewrite.
//!
//! # Why these and not four crypto tickers
//!
//! They exercise different halves of the SDK. Coinbase is a single-object
//! payload with a nanosecond-resolution event time and exact prices as
//! strings, and it is a cursor-less snapshot: every poll re-serves the same
//! last trade, so deduplication is what stops it being published repeatedly.
//! Frankfurter is a *fan-out*: one payload becomes one event per currency
//! pair, its event time is a date rather than an instant, and it has a real
//! sixteen-hour dissemination delay — so it is the one that shows event time,
//! ingest time and knowable time being three different things. Kalshi is a
//! fan-out whose refusals live in `map` rather than `decode`, so one bad
//! market is one quarantine and not a lost page, and whose two-sided book is
//! checked by exact decimal equality. Alpaca is the authenticated one, the
//! one whose prices arrive as JSON floats and must cross into `Decimal`
//! exactly, and the one whose event time is a session's midnight sixteen
//! hours before the close it reports.

pub mod alpaca_bars;
pub mod coinbase_ticker;
pub mod frankfurter_rates;
pub mod kalshi_markets;

pub use alpaca_bars::AlpacaBarsConnector;
pub use coinbase_ticker::CoinbaseTickerConnector;
pub use frankfurter_rates::FrankfurterRatesConnector;
pub use kalshi_markets::KalshiMarketsConnector;
