//! Seven worked connectors: five against endpoints that need no key and no
//! signup, and the two candidates ADR 0034 names for equities and
//! prediction markets.
//!
//! They are here rather than in a test file because an example nobody compiles
//! is an example that stops being true. All seven are ordinary
//! [`crate::connector::SourceConnector`] implementations, all seven ship their
//! manifest and a fixture, and all seven are run through
//! [`crate::connector::ContractHarness`] in `tests/connector_contract.rs` with
//! no network.
//!
//! | connector | source | what it produces |
//! |---|---|---|
//! | [`coinbase_ticker`] | `api.exchange.coinbase.com/products/BTC-USD/ticker` | a [`qip_market::quote::Tick`] per last trade |
//! | [`frankfurter_rates`] | `api.frankfurter.dev/v1/latest?base=EUR` | a [`qip_financial::intelligence::MacroObservation`] per currency pair |
//! | [`ecb_key_rates`] | `data-api.ecb.europa.eu/service/data/FM/...` | a [`qip_financial::intelligence::MacroObservation`] per key interest rate per date |
//! | [`nyfed_effr`] | `markets.newyorkfed.org/api/rates/unsecured/effr/last/10.json` | a [`qip_financial::intelligence::MacroObservation`] per business day |
//! | [`kalshi_markets`] | `api.elections.kalshi.com/trade-api/v2/markets` | a [`qip_market::quote::Quote`] per open binary market |
//! | [`alpaca_bars`] | `data.alpaca.markets/v2/stocks/bars` | a [`qip_market::bar::Bar`] per symbol per session |
//! | [`nws_station_observations`] | `api.weather.gov/stations/KORD/observations` | a [`qip_financial::intelligence::AlternativeDataPoint`] per graded reading per observation |
//!
//! # All seven are unreachable in this build, and say so
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
//! # Two of the seven are refused by the licensing gate today
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
//! hours before the close it reports. The ECB's key interest rates are the
//! one whose payload is a *cross product* — one message carrying several
//! series across several dates, indexed by position into dimension tables
//! rather than by name — so it is the connector that has to bound its own
//! decode before allocating, and the one whose response is held to a
//! **currency** because something downstream refuses on it. The New York
//! Fed's effective federal funds rate is the one whose payload carries **no**
//! currency at all, so the dollar is asserted from the identity of the rate
//! the path named rather than verified against the body; it is also the one
//! whose licence attaches an obligation to *presentation* rather than to
//! access, and the only one that reads a vendor's own revision flag instead of
//! declaring every row final.
//!
//! The National Weather Service's surface observations are the newest and the
//! first member of §7.1's **Physical** class, which had no connector at all.
//! It is the one whose publisher **grades its own readings** — every value
//! arrives with a quality-control letter — so it is the connector whose
//! `quality_of` carries a verdict rather than asserting one, and the only one
//! in which the source itself can tell this platform that a number it just
//! sent is wrong. It is also the one that refuses a timestamp whose UTC
//! offset is not zero, because `Timestamp::parse_rfc3339` discards an offset
//! and a reading filed five hours early is point-in-time leakage nothing
//! downstream could detect.

pub mod alpaca_bars;
pub mod coinbase_ticker;
pub mod ecb_key_rates;
pub mod frankfurter_rates;
pub mod kalshi_markets;
pub mod nws_station_observations;
pub mod nyfed_effr;

pub use alpaca_bars::AlpacaBarsConnector;
pub use coinbase_ticker::CoinbaseTickerConnector;
pub use ecb_key_rates::EcbKeyRatesConnector;
pub use frankfurter_rates::FrankfurterRatesConnector;
pub use kalshi_markets::KalshiMarketsConnector;
pub use nws_station_observations::NwsStationObservationsConnector;
pub use nyfed_effr::NyFedEffrConnector;
