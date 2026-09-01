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
use crate::connector::runtime::{ConnectorRuntime, RuntimeConfig};
use crate::connector::transport::{HttpSourceTransport, SourceTransport};
use crate::connector::{SourceConnector, manifest::SourceManifest};
use crate::connectors::CoinbaseTickerConnector;
use qip_core::error::{Error, Result};
use qip_core::{ObjectId, Timestamp};
use qip_events::Topic;

/// The sources this build can open by name.
///
/// A closed set, deliberately: opening a source is preceded by a licensing
/// evaluation, and an evaluation must name the thing it evaluated. A string
/// that could name any URL would let configuration reach past the catalogue.
pub const KNOWN_SOURCES: &[&str] = &[CoinbaseTickerConnector::SOURCE_ID];

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
        other => Err(Error::invalid(format!(
            "{other:?} names no connector this build carries; the known sources are: {}",
            KNOWN_SOURCES.join(", ")
        ))),
    }
}

/// A live connector, its transport and its runtime, behind the loop's own
/// adapter contract.
#[derive(Debug)]
pub struct ConnectorFeed {
    connector: Box<dyn SourceConnector>,
    transport: Box<dyn SourceTransport>,
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
        let (connector, mut manifest): (Box<dyn SourceConnector>, SourceManifest) = match source_id
        {
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
        connector: Box<dyn SourceConnector>,
        manifest: SourceManifest,
        transport: Box<dyn SourceTransport>,
        seed: u64,
        at: Timestamp,
    ) -> Result<Self> {
        let descriptor = SourceDescriptor {
            name: manifest.source_id.clone(),
            provider: manifest.provider.clone(),
            licensing: manifest.licensing,
            topics: vec![Topic::MarketTick],
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
    // not carry one; the composition root that owns the clock calls the
    // runtime's shutdown directly when it has an instant to give it.
}
