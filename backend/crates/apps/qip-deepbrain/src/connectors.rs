//! A catalogued connector, polled by the research node beside its own
//! stream, under a licensing gate that is re-asked before every poll.
//!
//! The fast brain has had this arm since ADR 0034 (`qip_fastbrain::feed::Feed::Connector`);
//! the deep brain did not, and ADR 0057's replay-under-admission was the
//! only way its ledger could hold a reference from a real vendor. The
//! review found that a replay's bytes are not a vendor's — a hand-written
//! file headed with a connector's name conferred that connector's standing
//! on whatever it held — so replayed bytes now count for no vendor, and this
//! arm is what lets the ledger hold a vendor's standing honestly: a fetch
//! this process made, from the vendor, digested where the bytes existed.
//!
//! # Additive, not a replacement
//!
//! The fast brain's connector *replaces* its synthetic exchange. Here the
//! arms are polled *beside* the engine's own stream, because the learning
//! desk fits on bars and no shipped connector emits one — the Coinbase
//! ticker ships ticks, the ECB rates ship macro observations, and Alpaca's
//! bars are refused by the catalogue until its terms are read. A deep brain
//! whose only stream was a tick connector would fit nothing and search
//! nothing. So the own stream (synthetic, replay or tape) keeps feeding the
//! desk, and each arm feeds the platform's observations and its reference
//! ledger, which is what rule 31 counts.
//!
//! # ADR 0024, and which brain can dial a vendor
//!
//! The deep brain carries the egress sidecar (`catalogue.tf`,
//! `egress_proxy = true`) and the fast brain deliberately does not (ADR
//! 0008: port 9102 on the proxy is a route to a language model). So the arm
//! the fast brain has always had is one it cannot use — `manifest_wiring.rs`
//! says so in its credential-mount rule — and this arm is the first on a
//! workload that can actually reach the vendor its manifest names. The
//! proxy's bootstrap names one of the two admitted connectors' hosts today
//! (`api.frankfurter.dev`, the `frankfurter` listener); Coinbase's is
//! deliberately absent, and selecting it is an edit to
//! `infrastructure/egress/envoy.yaml` reviewed with the tfvars that name it.
//!
//! # What a lapsed licence does
//!
//! The gate is re-asked at the instant of every poll, before the socket. A
//! refusal withdraws the source from the platform — its references stop
//! counting as live backing that instant — and is returned as the poll's
//! error, which stops the node loop, exactly as it stops the fast brain's:
//! an empty batch and a source this platform is no longer licensed to read
//! are indistinguishable downstream, and the second must stop the node
//! rather than quietly starve it. The replay-under-admission path takes the
//! other posture (a refused *round*, on the round line) because a replay is
//! a file this repository owns; a vendor is not.

use qip_core::Timestamp;
use qip_core::error::Result;
use qip_core::kv::KeyValueStore;
use qip_data_finder::admission::{AdmittedSource, CatalogueEntry, StandingAdmission};
use qip_data_finder::registration::RegistrationRegistry;
use qip_kernel::Platform;
use qip_market_ingestion::adapter::{DataAdapter, SensedRecord};
use qip_market_ingestion::connector::SourceConnector;
use qip_market_ingestion::connector::journal::StreamLedger;
use qip_market_ingestion::connector::manifest::SourceManifest;
use qip_market_ingestion::connector::transport::SourceTransport;
use qip_market_ingestion::connector_feed::ConnectorFeed;
use std::sync::Arc;

/// One catalogued connector, its standing gate, and the admission the
/// platform's reference ledger names the licence by.
#[derive(Debug)]
pub struct ConnectorArm {
    feed: ConnectorFeed,
    admission: StandingAdmission,
    /// The gate's latest decision bound to the manifest — what
    /// `Platform::admit_source` is handed before every poll.
    admitted: AdmittedSource,
}

impl ConnectorArm {
    /// Open a catalogued source through the egress proxy at `base_url`.
    ///
    /// The gate runs first, before any socket is touched, against the
    /// registry the node's configuration stands for; a source the catalogue
    /// refuses never has a transport built for it.
    pub fn open(
        source_id: &str,
        base_url: &str,
        registrations: &RegistrationRegistry,
        seed: u64,
        at: Timestamp,
    ) -> Result<Self> {
        let class = qip_market_ingestion::connector_feed::shipped_class(source_id)?;
        let (admission, decision) =
            StandingAdmission::open(registrations.clone(), source_id, class, at)?;
        let mut feed = ConnectorFeed::open(source_id, base_url, seed, at)?;
        // `feed.manifest()` is borrowed by `from_decision`'s error path too,
        // so the failure is matched rather than `?`-propagated: a refusal
        // here leaves the socket `ConnectorFeed::open` already holds with no
        // other path back to this function's stack frame, and until
        // 2026-09-12 that socket was released only by `Drop`, silently,
        // rather than by `ConnectorFeed::shutdown` — the same release every
        // exit past this constructor uses (`with_release` in `main.rs`).
        // Inert today (every shipped `SourceConnector::shutdown` is the
        // trait's no-op default), but the admission gate is exactly the
        // check a lapsed or misdeclared licence trips, so this is the one
        // failure arm most likely to be exercised in practice.
        match AdmittedSource::from_decision(&decision, feed.manifest()) {
            Ok(admitted) => Ok(Self {
                feed,
                admission,
                admitted,
            }),
            Err(error) => {
                let _ = feed.shutdown(at);
                Err(error)
            }
        }
    }

    /// The same assembly over a caller-supplied transport, through the real
    /// catalogue: how a test drives the arm against the recorded emulator
    /// with no socket, through the identical runtime path a deployment
    /// takes.
    pub fn over_transport(
        connector: Box<dyn SourceConnector + Send>,
        manifest: SourceManifest,
        transport: Box<dyn SourceTransport + Send>,
        registrations: &RegistrationRegistry,
        seed: u64,
        at: Timestamp,
    ) -> Result<Self> {
        let entries = qip_data_finder::admission::catalogue()?;
        Self::over_transport_admitted_by(
            &entries,
            connector,
            manifest,
            transport,
            registrations,
            seed,
            at,
        )
    }

    /// The same, against a caller-supplied catalogue — for the one arm
    /// worth testing that the real catalogue cannot produce: a licence that
    /// lapses between two polls, since no shipped entry expires and putting
    /// an expiry into a real vendor's evaluation to satisfy a test is the
    /// opposite of what that file is for.
    #[allow(clippy::too_many_arguments)]
    pub fn over_transport_admitted_by(
        entries: &[CatalogueEntry],
        connector: Box<dyn SourceConnector + Send>,
        manifest: SourceManifest,
        transport: Box<dyn SourceTransport + Send>,
        registrations: &RegistrationRegistry,
        seed: u64,
        at: Timestamp,
    ) -> Result<Self> {
        let (admission, decision) = StandingAdmission::over(
            entries.to_vec(),
            registrations.clone(),
            &manifest.source_id,
            manifest.licensing,
            at,
        )?;
        let mut feed = ConnectorFeed::over_transport(connector, manifest, transport, seed, at)?;
        // Released on the way out, same as `Self::open` above: a refusal
        // here has no other path back to this stack frame, and unlike
        // `Self::open` the transport this constructor is handed is the one
        // this crate's own tests use to pin the invariant, so a leak here
        // was the one this arm's admission-refusal test could actually
        // catch.
        let admitted = match AdmittedSource::from_decision(&decision, feed.manifest()) {
            Ok(admitted) => admitted,
            Err(error) => {
                let _ = feed.shutdown(at);
                return Err(error);
            }
        };
        Ok(Self {
            feed,
            admission,
            admitted,
        })
    }

    pub fn source_id(&self) -> &str {
        self.admitted.source_id()
    }

    pub fn admitted(&self) -> &AdmittedSource {
        &self.admitted
    }

    /// Keep this stream's record on `store`, resuming the last process's
    /// position; see `ConnectorFeed::journal_to` for why this must precede
    /// the first poll and what a zero means.
    pub fn journal_to(&mut self, store: Arc<dyn KeyValueStore>) -> Result<usize> {
        self.feed.journal_to(store)
    }

    pub fn ledger(&self) -> Option<&StreamLedger> {
        self.feed.ledger()
    }

    /// The gate's and the admission's own account of themselves, for the
    /// banner — the check count is what distinguishes a gate consulted on
    /// every poll from one consulted at start-up.
    pub fn describe(&self) -> String {
        format!(
            "{}; {}",
            self.admission.describe(),
            self.admitted.describe()
        )
    }

    /// Ask the gate at `until`, copy its answer into the platform, and poll
    /// with the fetch referenced on the platform's ledger between the poll
    /// and the checkpoint commit. A refusal by the gate withdraws the source
    /// from the platform and is the poll's error; a refusal by the platform
    /// to reference the fetch unwinds the connector so the next poll
    /// re-fetches (`ConnectorFeed::poll_referencing`).
    pub fn poll(&mut self, platform: &mut Platform, until: Timestamp) -> Result<Vec<SensedRecord>> {
        match self
            .admission
            .check(until)
            .and_then(|decision| AdmittedSource::from_decision(&decision, self.feed.manifest()))
        {
            Ok(admitted) => {
                self.admitted = admitted.clone();
                if platform.admitted_source(admitted.source_id()) != Some(&admitted) {
                    platform.admit_source(admitted);
                }
            }
            Err(refusal) => {
                platform.withdraw_source(self.admitted.source_id());
                return Err(refusal);
            }
        }
        self.feed.poll_referencing(until, &mut |digest| {
            platform.reference_fetch(digest, until).map(|_| ())
        })
    }

    /// Release what `open` acquired, at an instant the caller owns.
    pub fn shutdown(&mut self, at: Timestamp) -> Result<()> {
        self.feed.shutdown(at)
    }

    pub fn descriptor(&self) -> qip_market_ingestion::adapter::SourceDescriptor {
        self.feed.descriptor()
    }
}

#[cfg(test)]
mod tests {
    //! Code review (should-fix): `Self::open`'s admission check ran after
    //! `ConnectorFeed::open` had already opened a transport, and a refusal
    //! there released nothing but `Drop` — silently, and never through
    //! `ConnectorFeed::shutdown`, the release every exit past this
    //! constructor uses (`with_release` in `main.rs`). Inert today, since
    //! every shipped `SourceConnector::shutdown` is the trait's no-op
    //! default, but the admission gate is exactly what a lapsed or
    //! misdeclared licence trips.
    //!
    //! `Self::open` itself opens a real socket through the egress proxy and
    //! cannot be driven from a unit test; `Self::over_transport_admitted_by`
    //! runs the identical two-step construction — admit, then open, then
    //! `AdmittedSource::from_decision` — over a caller-supplied transport,
    //! so this test drives that seam instead and pins the same `match`
    //! shape `Self::open` now shares with it.

    // The workspace denies `panic_in_result_fn` for production code; in a
    // test the assertion is the deliverable and `?` keeps the setup readable.
    #![allow(clippy::panic_in_result_fn)]

    use super::*;
    use qip_market_ingestion::connector::checkpoint::Cursor;
    use qip_market_ingestion::connector::emulator::SourceEmulator;
    use qip_market_ingestion::connector::envelope::RawEvent;
    use qip_market_ingestion::connectors::CoinbaseTickerConnector;
    use std::sync::Mutex;

    /// A connector that records whether it was shut down, and otherwise
    /// answers nothing: decoding and mapping are never reached by this test,
    /// since the failure under test happens before the first poll.
    #[derive(Debug)]
    struct ShutdownSpy {
        manifest: SourceManifest,
        shut_down: Arc<Mutex<bool>>,
    }

    impl SourceConnector for ShutdownSpy {
        fn manifest(&self) -> &SourceManifest {
            &self.manifest
        }

        fn decode(&self, _payload: &serde_json::Value, _cursor: &Cursor) -> Result<Vec<RawEvent>> {
            Ok(Vec::new())
        }

        fn map(&self, _event: &RawEvent, _ingest_time: Timestamp) -> Result<SensedRecord> {
            Err(qip_core::error::Error::invalid(
                "the spy decodes no events, so it maps none",
            ))
        }

        fn shutdown(&mut self, at: Timestamp) -> Result<()> {
            let _ = at;
            *self
                .shut_down
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            Ok(())
        }
    }

    /// A manifest `AdmittedSource::from_decision` refuses *after* the feed
    /// is open still releases the feed, through `ConnectorFeed::shutdown`
    /// rather than only through `Drop`.
    ///
    /// The admission gate itself must succeed for this to test the seam it
    /// claims to: the catalogue's real Coinbase entry, the manifest's own
    /// licensing class and both required usages all agree, so the refusal
    /// below comes from `from_decision`'s own category check and not from
    /// `StandingAdmission::over` refusing earlier — before any transport
    /// exists at all, which would prove nothing about this seam.
    ///
    /// Mutated by reverting the `match` in `over_transport_admitted_by` to
    /// `AdmittedSource::from_decision(&decision, feed.manifest())?` —
    /// confirmed the refusal still fires (the `?` still propagates it) but
    /// `*shut_down.lock()` reads `false`, and the assertion that the feed
    /// was released fails; restored.
    #[test]
    fn an_admission_refusal_after_the_feed_opens_still_releases_it() -> Result<()> {
        let at = Timestamp::from_secs(1_760_000_000);
        let mut manifest = CoinbaseTickerConnector::shipped_manifest()?;
        manifest.endpoint.base_url = Some("http://127.0.0.1:1".to_string());
        // No §7.6.1 category: the one failure `over_transport_admitted_by`
        // cannot avoid by construction, since `over_transport` — unlike
        // `ConnectorFeed::open` — never calls `SourceManifest::validate`.
        manifest.category = None;
        let shut_down = Arc::new(Mutex::new(false));
        let connector = ShutdownSpy {
            manifest: manifest.clone(),
            shut_down: shut_down.clone(),
        };
        let body = r#"{"ask":"64231.55","bid":"64230.11","volume":"9184.4","trade_id":712553481,"price":"64230.99","size":"0.0018","time":"2026-08-27T00:00:00Z"}"#;
        let transport = SourceEmulator::serving(manifest.endpoint.path.clone(), body);
        let entries = qip_data_finder::admission::catalogue()?;

        let error = ConnectorArm::over_transport_admitted_by(
            &entries,
            Box::new(connector),
            manifest,
            Box::new(transport),
            &RegistrationRegistry::shipped(),
            7,
            at,
        )
        .expect_err("a manifest declaring no §7.6.1 category was admitted");
        assert!(
            error.message().contains("category"),
            "the premise failed: this is not the category refusal this test means to drive: {}",
            error.message()
        );
        assert!(
            *shut_down
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            "the feed opened by `over_transport_admitted_by` was not released when the \
             admission check that followed refused it"
        );
        Ok(())
    }
}
