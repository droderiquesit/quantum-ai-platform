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
use crate::connector::digest::FetchDigest;
use crate::connector::journal::{StreamJournal, StreamLedger};
use crate::connector::runtime::{ConnectorRuntime, RuntimeConfig};
use crate::connector::transport::{HttpSourceTransport, SourceTransport};
use crate::connector::{SourceConnector, manifest::SourceManifest};
use crate::connectors::{
    AlpacaBarsConnector, CoinbaseTickerConnector, FrankfurterRatesConnector, KalshiMarketsConnector,
};
use qip_core::error::{Error, Result};
use qip_core::kv::KeyValueStore;
use qip_core::{ObjectId, Timestamp};
use qip_events::Topic;
use std::collections::BTreeMap;
use std::sync::Arc;

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
    /// The durable record of the stream, when a caller has given this feed a
    /// store to keep one on. `None` is a feed whose figures die with the
    /// process — the correct shape for a contract test driving an emulator,
    /// and the wrong one for anything measuring a source *sustained*.
    journal: Option<StreamJournal>,
    /// The digest of the most recent delivered poll, held until the
    /// composition root takes it with [`Self::take_digest`]. Held rather than
    /// returned because [`DataAdapter::poll`] returns records and nothing
    /// else, and widening that contract for one arm would put a field on
    /// every adapter that has no bytes to digest.
    last_digest: Option<FetchDigest>,
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
            journal: None,
            last_digest: None,
        })
    }

    /// The manifest this feed was opened from — the declared category, the
    /// schema contract and the licensing class a composition root needs to
    /// build the source's admission for the kernel's reference ledger.
    pub const fn manifest(&self) -> &SourceManifest {
        self.runtime.manifest()
    }

    /// The digest of the most recent delivered poll, once.
    ///
    /// `None` when the last poll was deferred, refused, or decoded to nothing,
    /// and after this has already been taken — a digest handed to the kernel
    /// twice would record one fetch as two.
    pub fn take_digest(&mut self) -> Option<FetchDigest> {
        self.last_digest.take()
    }

    /// Keep this stream's record on `store`, and resume where the last process
    /// left off.
    ///
    /// # The gap this closes
    ///
    /// Every piece of the restart story existed before this method and none of
    /// them met: [`StreamJournal`] recorded sessions, both time axes and the
    /// dedup ratio durably, [`Self::checkpoint`] could take a resume position
    /// and [`Self::resume`] could apply one — and the only caller that ever put
    /// the four calls in the right order was a test. The two composition roots
    /// that construct a `ConnectorFeed` called none of them, so every restart
    /// began with an empty dedup window and republished the source's whole
    /// table as new observations, and the figures an operator would need to
    /// claim a source had streamed for a week reset to zero at each start-up.
    /// A control correct in every part and assembled by nobody is not a
    /// control; it is the shape of one.
    ///
    /// So the ordering lives here, one seam below the root, where it cannot be
    /// got wrong by a root that forgets a step.
    ///
    /// # Why this must precede the first poll
    ///
    /// [`crate::connector::DedupWindow::restore`] refuses a window that has
    /// already observed something, so calling this after a poll returns that
    /// refusal rather than a partial restore. Refusing is the point: a feed
    /// that quietly restored half a window would suppress some redeliveries and
    /// republish others, and no downstream figure would show which.
    ///
    /// Returns the number of fingerprints taken from the previous session's
    /// checkpoint. Zero is not a failure — it is a first session, or a
    /// checkpoint that genuinely carried nothing — but it does mean the next
    /// poll will republish whatever the source re-serves, which is worth a log
    /// line at a root.
    pub fn journal_to(&mut self, store: Arc<dyn KeyValueStore>) -> Result<usize> {
        if self.journal.is_some() {
            return Err(Error::invalid(format!(
                "the `{}` feed already keeps a journal. Attaching a second would split one \
                 stream's record across two ledgers, and the session count each reported would \
                 be a fraction of the truth; open one journal per feed, at start-up",
                self.descriptor.name
            )));
        }
        let (journal, resumed) = StreamJournal::open(store, &self.descriptor.name)?;
        let taken = match resumed {
            Some(checkpoint) => self.runtime.resume(self.connector.as_mut(), &checkpoint)?,
            None => 0,
        };
        self.journal = Some(journal);
        Ok(taken)
    }

    /// What this stream has done across every process that carried it, when a
    /// journal is kept.
    ///
    /// `None` rather than an empty ledger for a feed with no store: a ledger of
    /// zeroes and a stream that has genuinely done nothing read identically,
    /// and the completion plan's seven-day bar is exactly the claim that
    /// confusion would corrupt.
    pub const fn ledger(&self) -> Option<&StreamLedger> {
        match &self.journal {
            Some(journal) => Some(journal.ledger()),
            None => None,
        }
    }

    /// Release what [`Self::open`] acquired, at an instant the caller owns.
    ///
    /// [`DataAdapter::stop`] carries no clock and the runtime's shutdown needs
    /// one, so the trait default stays a no-op here. What this replaces is a
    /// comment telling the composition root to call the runtime's shutdown
    /// directly — but `runtime` and `connector` are private and there was no
    /// accessor, so it named a call no root could make, and a node that
    /// stopped cleanly released the connector's session not at all.
    /// Nothing is committed here, deliberately. [`DataAdapter::poll`] commits
    /// the position on every poll, so the store already holds the window as of
    /// the last record; a second commit at shutdown would write the identical
    /// checkpoint and no test could tell whether it had run. A line no test can
    /// distinguish is a line that will one day be wrong without failing.
    pub fn shutdown(&mut self, at: Timestamp) -> Result<()> {
        self.runtime.shutdown(self.connector.as_mut(), at)
    }

    /// The cursor a restart would resume from, and the dedup window it would
    /// resume with.
    ///
    /// Exposed for the same reason: [`ConnectorRuntime::checkpoint`] is public
    /// and was unreachable through this bridge, so a restarted node had no
    /// cursor to resume from and would re-poll the manifest's whole window.
    pub fn checkpoint(&self, at: Timestamp) -> Checkpoint {
        self.runtime.checkpoint(at)
    }

    /// Restore the position and the dedup window the last process left.
    ///
    /// Returns the fingerprints taken. Call it before the first
    /// [`DataAdapter::poll`]: [`crate::connector::DedupWindow::restore`]
    /// refuses a window that has already observed something, so a resume after
    /// a poll is an error rather than a partial restore, and the error names
    /// what to do instead.
    ///
    /// This is the half of the restart story `checkpoint` could not tell on its
    /// own. Taking a checkpoint was reachable through this bridge and applying
    /// one was not, so a composition root could write a resume position it had
    /// no way to use — the same shape as a limit that cannot fire, one seam
    /// earlier.
    pub fn resume(&mut self, checkpoint: &Checkpoint) -> Result<usize> {
        self.runtime.resume(self.connector.as_mut(), checkpoint)
    }
}

impl DataAdapter for ConnectorFeed {
    fn descriptor(&self) -> SourceDescriptor {
        self.descriptor.clone()
    }

    fn poll(&mut self, until: Timestamp) -> Result<Vec<SensedRecord>> {
        let mut report =
            self.runtime
                .poll(self.connector.as_mut(), self.transport.as_mut(), until)?;
        // Taken before the journal writes, and replaced rather than kept: a
        // digest is a fact about *this* poll, and one left over from a poll
        // whose successor delivered nothing would be handed up as if it were
        // fresh. The topic is the bridge's to add — the runtime does not
        // know it — and it is the one the descriptor already promises.
        self.last_digest =
            report
                .digest
                .take()
                .map(|digest| match self.descriptor.topics.first() {
                    Some(topic) => digest.with_topic(*topic),
                    None => digest,
                });
        // Recorded before the records are released, and the error propagates
        // rather than being swallowed. A stream whose durable record cannot be
        // written is a stream nobody can afterwards say anything true about,
        // and the platform's own rule is that the louder of two disagreeing
        // claims is the wrong one. Nothing here retries: the checkpoint stays
        // where it was, so the next process re-fetches what this poll withheld.
        // Then the position, so a process killed between two polls resumes with
        // the window this poll built rather than with an empty one. Committing
        // per poll and not at shutdown is the whole point: the failures being
        // survived here are the ones that do not run a shutdown.
        //
        // The failure ordering is deliberate. A poll recorded whose checkpoint
        // did not commit reads as duplicates on the next run, which
        // `StreamLedger::duplicate_ratio` shows; a checkpoint committed for a
        // poll that was never recorded would be invisible.
        if let Some(journal) = self.journal.as_mut() {
            journal.record(&report, until)?;
        }
        let checkpoint = self.runtime.checkpoint(until);
        if let Some(journal) = self.journal.as_mut() {
            journal.commit(&checkpoint)?;
        }
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
