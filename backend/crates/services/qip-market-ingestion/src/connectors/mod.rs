//! Two worked connectors, against endpoints that need no key and no signup.
//!
//! They are here rather than in a test file because an example nobody compiles
//! is an example that stops being true. Both are ordinary
//! [`crate::connector::SourceConnector`] implementations, both ship their
//! manifest and a recorded fixture, and both are run through
//! [`crate::connector::ContractHarness`] in `tests/connector_contract.rs` with
//! no network.
//!
//! | connector | source | what it produces |
//! |---|---|---|
//! | [`coinbase_ticker`] | `api.exchange.coinbase.com/products/BTC-USD/ticker` | a [`qip_market::quote::Tick`] per last trade |
//! | [`frankfurter_rates`] | `api.frankfurter.dev/v1/latest?base=EUR` | a [`qip_financial::intelligence::MacroObservation`] per currency pair |
//!
//! # Both are unreachable in this build, and say so
//!
//! `qip_transport::http` has no TLS stack and refuses `https` by name rather
//! than downgrading it. Both endpoints are HTTPS only. So both manifests ship
//! with **no `base_url`**, which makes
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
//! # Why these two and not two crypto tickers
//!
//! They exercise different halves of the SDK. Coinbase is a single-object
//! payload with a nanosecond-resolution event time and exact prices as
//! strings, and it is a cursor-less snapshot: every poll re-serves the same
//! last trade, so deduplication is what stops it being published repeatedly.
//! Frankfurter is a *fan-out*: one payload becomes one event per currency
//! pair, its event time is a date rather than an instant, and it has a real
//! sixteen-hour dissemination delay — so it is the one that shows event time,
//! ingest time and knowable time being three different things.

pub mod coinbase_ticker;
pub mod frankfurter_rates;

pub use coinbase_ticker::CoinbaseTickerConnector;
pub use frankfurter_rates::FrankfurterRatesConnector;
