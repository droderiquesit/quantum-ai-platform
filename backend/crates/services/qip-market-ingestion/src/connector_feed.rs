//! The connector SDK as a [`DataAdapter`], so a worked connector can drive the
//! decision loop.
//!
//! The SDK's runtime speaks in [`MarketEventEnvelope`]s and the loop's feed
//! speaks in [`SensedRecord`]s; this is the bridge, and it is deliberately
//! thin. Everything with judgement in it — rate limits, retries, dedup on the
//! stable fingerprint, knowability withholding, quarantine — stays in
//! [`ConnectorRuntime`], where its own tests live. A bridge that re-made any
//! of those decisions would be a second ingestion discipline wearing the
//! first one's name.
//!
//! # The egress address is the deployment's, never the manifest's
//!
//! Shipped manifests carry no `base_url`, because the workspace transport
//! speaks plaintext HTTP/1.1 and the worked sources are HTTPS-only: the
//! address of a TLS-terminating egress proxy is deployment configuration, and
//! [`ConnectorFeed::open`] takes it as an argument and refuses an `https`
//! address by name rather than downgrading it — a fallback to plaintext
//! against the vendor would put every request on the wire in clear.

use crate::adapter::{DataAdapter, SensedRecord, SourceDescriptor};
use crate::connector::checkpoint::Checkpoint;
use crate::connector::runtime::{ConnectorRuntime, RuntimeConfig};
use crate::connector::transport::{HttpSourceTransport, SourceTransport};
use crate::connector::{SourceConnector, manifest::SourceManifest};
use crate::connectors::{
    AlpacaBarsConnector, CoinbaseTickerConnector, FrankfurterRatesConnector, KalshiMarketsConnector,
};
use qip_core::error::{Error, Result};
use qip_core::{ObjectId, Timestamp};
use qip_events::Topic;
use std::collections::BTreeMap;

/// The sources this build can open by name.
///
/// A closed set, deliberately: opening a source is preceded by a licensing
/// evaluation, and an evaluation must name the thing it evaluated. A string
/// that could name any URL would let configuration reach past the catalogue.
///
/// Being named here is not being admitted: Kalshi and Alpaca are ADR 0034
/// candidates whose terms are unread, and `qip_data_finder::admission::admit`
/// refuses both. They are listed so that a deployment can select them the
/// day the gate admits them, and so the catalogue's own integrity check —
/// an entry for a source no build carries is decoration — holds for them.
pub const KNOWN_SOURCES: &[&str] = &[
    CoinbaseTickerConnector::SOURCE_ID,
    FrankfurterRatesConnector::SOURCE_ID,
    KalshiMarketsConnector::SOURCE_ID,
    AlpacaBarsConnector::SOURCE_ID,
];

/// The symbols the shipped Alpaca manifest fetches, each with the instrument
/// it maps to. Hard-coded beside Coinbase's `BTC-USD` for the same reason:
/// the mapping is the composition root's decision and the root does not
/// carry one yet.
fn alpaca_instruments() -> BTreeMap<String, ObjectId> {
    ["AAPL", "MSFT"]
        .into_iter()
        .map(|symbol| (symbol.to_string(), ObjectId::from_string(symbol)))
        .collect()
}

/// The topic a named source's records are published under.
///
/// A [`SourceDescriptor`] that claimed [`Topic::MarketTick`] for a connector
/// that actually emits [`crate::adapter::SensedRecord::Macro`] would tell a
/// consumer reading the descriptor to expect a topic that never arrives —
/// the mistake this function exists to make impossible to copy-paste into a
/// second connector, which is exactly how it reached this bridge in the
/// first place: [`Self::over_transport`] used to hard-code
/// [`Topic::MarketTick`] for every source.
fn topic_for(source_id: &str) -> Result<Topic> {
    match source_id {
        CoinbaseTickerConnector::SOURCE_ID => Ok(Topic::MarketTick),
        FrankfurterRatesConnector::SOURCE_ID => Ok(Topic::MacroUpdated),
        KalshiMarketsConnector::SOURCE_ID => Ok(Topic::MarketQuote),
        AlpacaBarsConnector::SOURCE_ID => Ok(Topic::MarketBar),
        other => Err(Error::invalid(format!(
            "{other:?} names no connector this build carries; the known sources are: {}",
            KNOWN_SOURCES.join(", ")
        ))),
    }
}

/// The licensing class a named source's shipped manifest declares.
///
/// For the gate that must run *before* the source is opened: the caller
/// compares this against its catalogue's evaluation, and a disagreement
/// between the two claims refuses the source. Reading it does not construct a
/// connector and touches no socket.
pub fn shipped_class(source_id: &str) -> Result<qip_financial::quality::LicensingClass> {
    match source_id {
        CoinbaseTickerConnector::SOURCE_ID => {
            Ok(CoinbaseTickerConnector::shipped_manifest()?.licensing)
        }
        FrankfurterRatesConnector::SOURCE_ID => {
            Ok(FrankfurterRatesConnector::shipped_manifest()?.licensing)
        }
        KalshiMarketsConnector::SOURCE_ID => {
            Ok(KalshiMarketsConnector::shipped_manifest()?.licensing)
        }
        AlpacaBarsConnector::SOURCE_ID => Ok(AlpacaBarsConnector::shipped_manifest()?.licensing),
        other => Err(Error::invalid(format!(
            "{other:?} names no connector this build carries; the known sources are: {}",
            KNOWN_SOURCES.join(", ")
        ))),
    }
}

/// A live connector, its transport and its runtime, behind the loop's own
/// adapter contract.
///
/// Both trait objects are `Send`, and the bound is load-bearing rather than
/// decorative: `qip-api` holds its feed behind a mutex that every request
/// thread can reach, and a `Mutex<T>` is shareable only when `T` can move
/// between threads. Every connector and transport this crate ships is plain
/// data and satisfies it; a future one holding a thread-local handle would
/// be refused here at compile time rather than discovered as a data race in
/// a request handler.
#[derive(Debug)]
pub struct ConnectorFeed {
    connector: Box<dyn SourceConnector + Send>,
    transport: Box<dyn SourceTransport + Send>,
    runtime: ConnectorRuntime,
    descriptor: SourceDescriptor,
}

impl ConnectorFeed {
    /// Open a named source through the egress proxy at `base_url`.
    ///
    /// The caller is expected to have run the licensing gate first — the
    /// composition root does, and refuses to construct this without a
    /// permitted assessment — but the manifest still travels with its own
    /// licensing class and the descriptor repeats it, so a record's
    /// provenance says what its source's terms were wherever it ends up.
    pub fn open(source_id: &str, base_url: &str, seed: u64, at: Timestamp) -> Result<Self> {
        if base_url.starts_with("https://") {
            return Err(Error::invalid(format!(
                "the connector egress address is {base_url}. This transport speaks plaintext \
                 HTTP/1.1 and has no TLS stack: point it at the egress proxy that terminates \
                 TLS to the vendor, never at the vendor itself"
            )));
        }
        let (connector, mut manifest): (Box<dyn SourceConnector + Send>, SourceManifest) =
            match source_id {
                CoinbaseTickerConnector::SOURCE_ID => {
                    let manifest = CoinbaseTickerConnector::shipped_manifest()?;
                    let connector = CoinbaseTickerConnector::new(
                        manifest.clone(),
                        "BTC-USD",
                        ObjectId::from_string("BTC-USD"),
                        "COINBASE",
                    )?;
                    (Box::new(connector), manifest)
                }
                FrankfurterRatesConnector::SOURCE_ID => {
                    let manifest = FrankfurterRatesConnector::shipped_manifest()?;
                    let connector = FrankfurterRatesConnector::new(manifest.clone())?;
                    (Box::new(connector), manifest)
                }
                KalshiMarketsConnector::SOURCE_ID => {
                    let manifest = KalshiMarketsConnector::shipped_manifest()?;
                    let connector = KalshiMarketsConnector::new(manifest.clone())?;
                    (Box::new(connector), manifest)
                }
                AlpacaBarsConnector::SOURCE_ID => {
                    let manifest = AlpacaBarsConnector::shipped_manifest()?;
                    let connector =
                        AlpacaBarsConnector::new(manifest.clone(), alpaca_instruments())?;
                    (Box::new(connector), manifest)
                }
                other => {
                    return Err(Error::invalid(format!(
                        "{other:?} names no connector this build carries. The known sources are: {}. \
                     A source outside this list has no licensing evaluation on file, and an \
                     unevaluated source is refused rather than fetched",
                        KNOWN_SOURCES.join(", ")
                    )));
                }
            };
        manifest.endpoint.base_url = Some(base_url.to_string());
        manifest.validate()?;

        let transport = Box::new(HttpSourceTransport::connect(&manifest)?);
        Self::over_transport(connector, manifest, transport, seed, at)
    }

    /// The same assembly over a caller-supplied transport.
    ///
    /// This is how the contract tests drive the bridge against the recorded
    /// emulator with no socket, through the identical runtime path a
    /// deployment takes — the only difference between a test and production
    /// is the transport, which is the difference it should be.
    pub fn over_transport(
        connector: Box<dyn SourceConnector + Send>,
        manifest: SourceManifest,
        transport: Box<dyn SourceTransport + Send>,
        seed: u64,
        at: Timestamp,
    ) -> Result<Self> {
        let topic = topic_for(&manifest.source_id)?;
        let descriptor = SourceDescriptor {
            name: manifest.source_id.clone(),
            provider: manifest.provider.clone(),
            licensing: manifest.licensing,
            topics: vec![topic],
            expected_latency: manifest.poll_interval(),
            production_requirement: None,
        };
        let mut runtime = ConnectorRuntime::new(manifest, RuntimeConfig::seeded(seed))?;
        let mut boxed = connector;
        let mut transport = transport;
        runtime.connect(boxed.as_mut(), transport.as_mut(), at)?;
        Ok(Self {
            connector: boxed,
            transport,
            runtime,
            descriptor,
        })
    }

    /// Release what [`Self::open`] acquired, at an instant the caller owns.
    ///
    /// [`DataAdapter::stop`] carries no clock and the runtime's shutdown needs
    /// one, so the trait default stays a no-op here. What this replaces is a
    /// comment telling the composition root to call the runtime's shutdown
    /// directly — but `runtime` and `connector` are private and there was no
    /// accessor, so it named a call no root could make, and a node that
    /// stopped cleanly released the connector's session not at all.
    pub fn shutdown(&mut self, at: Timestamp) -> Result<()> {
        self.runtime.shutdown(self.connector.as_mut(), at)
    }

    /// The cursor a restart would resume from.
    ///
    /// Exposed for the same reason: [`ConnectorRuntime::checkpoint`] is public
    /// and was unreachable through this bridge, so a restarted node had no
    /// cursor to resume from and would re-poll the manifest's whole window.
    pub fn checkpoint(&self, at: Timestamp) -> Checkpoint {
        self.runtime.checkpoint(at)
    }
}

impl DataAdapter for ConnectorFeed {
    fn descriptor(&self) -> SourceDescriptor {
        self.descriptor.clone()
    }

    fn poll(&mut self, until: Timestamp) -> Result<Vec<SensedRecord>> {
        let report = self
            .runtime
            .poll(self.connector.as_mut(), self.transport.as_mut(), until)?;
        Ok(report
            .admitted
            .into_iter()
            .map(|envelope| envelope.into_record())
            .collect())
    }

    // `stop` keeps the trait default. The runtime's own shutdown wants the
    // caller's clock for its final checkpoint, and the adapter contract does
    // not carry one; the root that owns the clock calls
    // [`ConnectorFeed::shutdown`], which this type exposes for exactly that.
}

#[cfg(test)]
mod tests {
    //! Beside the code rather than in `tests/`: the property under test is
    //! that this bridge forwards the caller's instant into the runtime it
    //! privately holds, and `runtime` and `connector` are private fields no
    //! integration test can see either side of.

    use super::*;
    use crate::connector::checkpoint::Cursor;
    use crate::connector::emulator::SourceEmulator;
    use crate::connector::envelope::RawEvent;
    use std::sync::{Arc, Mutex};

    /// A connector that records the instant it was shut down at, and decodes
    /// nothing. `shutdown` is the only lifecycle call under test, and a real
    /// connector's is a no-op that would leave nothing to assert on. The
    /// shared cell is an `Arc<Mutex>` rather than an `Rc<Cell>` because the
    /// feed now demands a `Send` connector, and this spy is the one caller
    /// that would otherwise have been refused by the bound it exists to
    /// prove nothing real is refused by.
    #[derive(Debug)]
    struct ShutdownSpy {
        manifest: SourceManifest,
        shut_down_at: Arc<Mutex<Option<Timestamp>>>,
    }

    impl ShutdownSpy {
        fn read(cell: &Mutex<Option<Timestamp>>) -> Option<Timestamp> {
            *cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }

    impl SourceConnector for ShutdownSpy {
        fn manifest(&self) -> &SourceManifest {
            &self.manifest
        }

        fn decode(&self, _payload: &serde_json::Value, _cursor: &Cursor) -> Result<Vec<RawEvent>> {
            Ok(Vec::new())
        }

        fn map(&self, _event: &RawEvent, _ingest_time: Timestamp) -> Result<SensedRecord> {
            Err(Error::invalid("the spy decodes no events, so it maps none"))
        }

        fn shutdown(&mut self, at: Timestamp) -> Result<()> {
            *self
                .shut_down_at
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(at);
            Ok(())
        }
    }

    fn instant(text: &str) -> Timestamp {
        Timestamp::parse_rfc3339(text).expect("a literal RFC 3339 instant")
    }

    #[test]
    fn the_bridge_carries_the_callers_instant_into_the_runtimes_shutdown_and_checkpoint() {
        let opened = instant("2026-08-27T00:00:00Z");
        let closed = instant("2026-08-27T06:00:00Z");
        assert_ne!(
            opened, closed,
            "the two instants must differ, or forwarding the wrong one would still pass"
        );

        let mut manifest =
            FrankfurterRatesConnector::shipped_manifest().expect("the shipped manifest parses");
        manifest.endpoint.base_url = Some("http://127.0.0.1:1".to_string());
        let health_path = manifest.endpoint.health_path().to_string();
        let transport = Box::new(SourceEmulator::serving(
            health_path,
            r#"{"amount":1.0,"base":"EUR","date":"2026-08-24","rates":{"USD":1.0827}}"#,
        ));
        let shut_down_at = Arc::new(Mutex::new(None));
        let connector = Box::new(ShutdownSpy {
            manifest: manifest.clone(),
            shut_down_at: shut_down_at.clone(),
        });

        let mut feed = ConnectorFeed::over_transport(connector, manifest, transport, 7, opened)
            .expect("the emulator answers the health probe");

        // Premise: nothing has been shut down yet, so the assertion below is
        // this call's doing and not the constructor's.
        assert_eq!(ShutdownSpy::read(&shut_down_at), None);

        feed.shutdown(closed).expect("the spy cannot fail to stop");
        assert_eq!(
            ShutdownSpy::read(&shut_down_at),
            Some(closed),
            "shutdown must reach the connector at the instant the caller gave"
        );

        let checkpoint = feed.checkpoint(closed);
        assert_eq!(
            checkpoint.taken_at, closed,
            "a checkpoint stamped with anything but the caller's instant would resume from a \
             position nobody asked for"
        );
        assert_eq!(checkpoint.source_id, "frankfurter-ecb-reference-rates");
    }
}
