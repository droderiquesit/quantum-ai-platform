//! Which sources this node will open, on whose registration, and what it
//! refuses at start.
//!
//! The failure guarded here is a node reading a venue's data on nobody's
//! account. `qip-fastbrain` used to admit its connector through
//! `qip_data_finder::admission::admit`, which consults the shipped
//! requirement table and holds no record: an account-gated source could never
//! be opened however the deployment was configured, and the refusal named a
//! table nothing could add to. It now admits through `admit_registered`
//! against the registry `PlatformConfig::registration_registry` builds — the
//! same registry the platform is assembled with — so the owner's recorded
//! registration is what changes the answer, and nothing else is.
//!
//! Every test asserts its premise before its property: that the registry
//! *did* demand an account, that the licensing questions *did* pass, that the
//! keyless source *is* declared keyless. Without those, a gate that refused
//! everything would satisfy the refusals and prove nothing.

// The workspace denies `panic_in_result_fn` for production code, where an
// assertion that aborts instead of returning is a bug. In a test the assertion
// is the deliverable and `?` is what keeps the setup readable.
#![allow(clippy::panic_in_result_fn)]

use qip_contracts::governance::Usage;
use qip_core::error::Result;
use qip_core::{Duration, Timestamp};
use qip_data_finder::admission::{self, CatalogueEntry};
use qip_data_finder::legal::{LicensingPosture, SourceLicense};
use qip_data_finder::registration::{
    NOT_OFFERED, RegistrationRecord, RegistrationRegistry, RegistrationStanding,
};
use qip_fastbrain::config::ConnectorFeedSettings;
use qip_fastbrain::feed::{Feed, source_standings};
use qip_financial::quality::LicensingClass;
use qip_kernel::PlatformConfig;
use qip_market_ingestion::connector::manifest::SecretRef;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// --- fixtures -----------------------------------------------------------------

/// A source the shipped table says needs an account with the venue.
const ACCOUNT_SOURCE: &str = "alpaca-daily-bars";
/// The deployment variable the account source's manifest reads its credential
/// from. A name, never a value — `SecretRef` refuses anything key-shaped.
const ACCOUNT_SLOT: &str = "QIP_ALPACA_API_SECRET_KEY";
const TERMS: &str = "https://alpaca.markets/terms-and-conditions";
const OWNER: &str = "desk-owner";

/// A source the shipped table says is keyless, and whose manifest declares
/// `auth: none`, so it opens with no credential anywhere in the process.
const KEYLESS_SOURCE: &str = "frankfurter-ecb-reference-rates";

/// An address the transport refuses as its first act, before it resolves a
/// credential or opens a socket. Used as a sentinel: a refusal naming the
/// transport is proof that both gates passed the source through to it, and it
/// needs no network, no listener and no variable in the environment.
const TRANSPORT_SENTINEL: &str = "https://the-transport-has-no-tls-stack.invalid";

fn start() -> Timestamp {
    Timestamp::from_secs(1_760_000_000)
}

/// The account source with its terms read and every usage granted — an entry
/// the real catalogue must never contain, so that the licensing questions all
/// pass and the only gate left standing is the registration one. Without it
/// the account-gated sources are refused on their unread terms (ADR 0034) and
/// the registration refusal is unreachable.
fn terms_read() -> Result<Vec<CatalogueEntry>> {
    Ok(vec![CatalogueEntry {
        source_id: ACCOUNT_SOURCE,
        expected_class: LicensingClass::Restricted,
        posture: LicensingPosture::declared(SourceLicense::new(
            "alpaca-terms-as-read",
            [Usage::Research, Usage::Derive, Usage::Trade],
        )?),
    }])
}

fn account_settings(base_url: &str) -> ConnectorFeedSettings {
    ConnectorFeedSettings {
        source_id: ACCOUNT_SOURCE.to_string(),
        base_url: base_url.to_string(),
        seed: 7,
    }
}

/// The registry a deployment that recorded the owner's registration stands
/// for, built the way the composition root builds it.
fn registered() -> Result<RegistrationRegistry> {
    PlatformConfig::default()
        .with_venue_registrations(vec![RegistrationRecord::new(
            ACCOUNT_SOURCE,
            OWNER,
            start(),
            TERMS,
            SecretRef::new(ACCOUNT_SLOT)?,
        )?])
        .registration_registry()
}

/// The body the shipped Frankfurter fixture records from the live endpoint.
const RATE_TABLE: &str = r#"{"amount":1.0,"base":"EUR","date":"2026-08-24","rates":{"GBP":0.84215,"JPY":171.94,"USD":1.0827}}"#;

/// A loopback HTTP/1.1 server answering every request with one JSON body and
/// counting the connections it accepted.
///
/// The count is the witness the admission test needs: a source that was
/// admitted opens a socket, and one that was refused opens none, so "it was
/// admitted" is an observation rather than an inference from an `Ok`.
struct RateServer {
    url: String,
    served: Arc<AtomicUsize>,
}

impl RateServer {
    fn serving(body: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let url = format!(
            "http://{}",
            listener
                .local_addr()
                .expect("the listener has a local address")
        );
        let served = Arc::new(AtomicUsize::new(0));
        let counter = served.clone();
        // Detached: the listener lives as long as the test process, and a
        // request arriving after the test is answered and ignored rather than
        // left to hang the client.
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else {
                    break;
                };
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                while stream.read(&mut byte).is_ok_and(|n| n == 1) {
                    request.push(byte[0]);
                    if request.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self { url, served }
    }

    fn served(&self) -> usize {
        self.served.load(Ordering::SeqCst)
    }
}

// --- the gate -----------------------------------------------------------------

#[test]
fn a_configuration_carrying_the_owners_registration_record_admits_the_account_gated_source()
-> Result<()> {
    // Premise: the shipped table demands an account for this source and holds
    // no record, so there is something for the owner's record to change.
    let shipped = RegistrationRegistry::shipped();
    assert!(
        shipped
            .requirement(ACCOUNT_SOURCE)
            .is_some_and(|requirement| requirement.needs_registration()),
        "{ACCOUNT_SOURCE} is not account-gated in the shipped table, so this test proves nothing"
    );
    assert!(shipped.record(ACCOUNT_SOURCE).is_none());

    // Premise: the registry the configuration stands for holds the owner's
    // record, under the owner's name.
    let registrations = registered()?;
    match registrations.standing(ACCOUNT_SOURCE)? {
        RegistrationStanding::Registered { record } => {
            assert_eq!(record.operator(), OWNER);
            assert_eq!(record.terms(), TERMS);
        }
        RegistrationStanding::Keyless => {
            panic!("the account source stood as keyless, so the record was never applied")
        }
    }

    // The node's own door, with the registry its configuration stands for and
    // a catalogue under which every licensing question passes. What comes back
    // is the transport refusing the sentinel address — which it can only do
    // after both gates handed the source to it.
    let settings = account_settings(TRANSPORT_SENTINEL);
    let error = Feed::connector_admitted_by(&terms_read()?, &registrations, &settings, start())
        .expect_err("a transport with no TLS stack accepted an https address");
    assert!(
        !error.message().contains(NOT_OFFERED),
        "the owner's record did not satisfy the registration gate: {}",
        error.message()
    );
    assert!(
        error.message().contains("speaks plaintext HTTP/1.1"),
        "the refusal is not the transport's, so the gate never passed the source on: {}",
        error.message()
    );
    Ok(())
}

#[test]
fn without_the_owners_record_the_account_gated_source_is_refused_at_start_by_name() -> Result<()> {
    // Premise: a shipped deployment commits no registration, so this is the
    // state a node starts in until the owner records one.
    assert!(
        PlatformConfig::default().venue_registrations.is_empty(),
        "the shipped configuration registers somebody, which only the owner may do"
    );
    let registrations = PlatformConfig::default().registration_registry()?;
    assert!(registrations.record(ACCOUNT_SOURCE).is_none());

    // Premise: under this catalogue the licensing questions all pass, so the
    // refusal below is the registration question's answer and not the
    // licence's — the account-gated sources are refused on unread terms in
    // the real catalogue, and that refusal would look like a pass here.
    let entries = terms_read()?;
    assert!(
        entries[0]
            .posture
            .legality_for(Usage::Trade, start())
            .is_permitted()
    );

    // The same address the admitting test uses. It is never reached: the gate
    // refuses before anything is constructed, which is why no listener is
    // needed to run this.
    let settings = account_settings(TRANSPORT_SENTINEL);
    let refused = Feed::connector_admitted_by(&entries, &registrations, &settings, start())
        .expect_err("a source needing an account was opened with nobody registered");
    let message = refused.message();
    assert!(
        message.contains(&format!(
            "`{ACCOUNT_SOURCE}` requires an account with the venue"
        )),
        "the refusal does not name the source and its requirement: {message}"
    );
    assert!(
        message.contains("requirement `account`"),
        "the refusal does not name the declared requirement: {message}"
    );
    assert!(
        message.contains("owner must register with the venue under their own identity"),
        "the refusal does not say who must register: {message}"
    );
    assert!(
        message.contains(NOT_OFFERED),
        "the refusal does not say anonymous registration is not offered: {message}"
    );
    assert!(
        !message.contains("speaks plaintext HTTP/1.1"),
        "the transport was reached, so the gate ran after construction: {message}"
    );
    Ok(())
}

#[test]
fn a_keyless_source_is_admitted_through_the_nodes_own_door_as_it_was_before() -> Result<()> {
    // Premise: the registry a shipped configuration stands for declares this
    // source keyless, so what is proven below is the keyless arm and not a
    // record nobody made.
    let registrations = PlatformConfig::default().registration_registry()?;
    assert_eq!(
        registrations.standing(KEYLESS_SOURCE)?,
        RegistrationStanding::Keyless
    );

    let server = RateServer::serving(RATE_TABLE);
    // Premise: nothing has been asked of the server yet, so the count below
    // is this feed's socket and not a leftover.
    assert_eq!(server.served(), 0);

    // `Feed::open` is the door `main` takes, against the real licensing
    // catalogue: two gates, then a socket.
    let settings = ConnectorFeedSettings {
        source_id: KEYLESS_SOURCE.to_string(),
        base_url: server.url.clone(),
        seed: 7,
    };
    let feed = Feed::open(
        None,
        Some(&settings),
        None,
        None,
        &registrations,
        7,
        Duration::from_secs(1),
        start(),
    )?;
    assert!(
        matches!(feed, Feed::Connector { .. }),
        "a configured connector was replaced by another source"
    );
    assert_eq!(feed.descriptor().name, KEYLESS_SOURCE);
    assert!(
        server.served() >= 1,
        "the admitted source opened no socket, so nothing was actually admitted"
    );
    Ok(())
}

// --- the banner ---------------------------------------------------------------

#[test]
fn the_banner_states_every_catalogued_sources_standing_including_the_refused_ones() -> Result<()> {
    // Premise: the catalogue carries both kinds, so a banner that reported
    // only one kind would still have something to omit.
    let catalogue = admission::catalogue()?;
    assert!(
        catalogue
            .iter()
            .any(|entry| entry.source_id == ACCOUNT_SOURCE)
    );
    assert!(
        catalogue
            .iter()
            .any(|entry| entry.source_id == KEYLESS_SOURCE)
    );

    let unregistered = PlatformConfig::default().registration_registry()?;
    let lines = source_standings(&unregistered)?;
    assert_eq!(
        lines.len(),
        catalogue.len(),
        "the banner does not carry one line per catalogued source: {lines:?}"
    );
    for entry in &catalogue {
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains(&format!(" {}: ", entry.source_id)))
                .count(),
            1,
            "{} is named other than once in the banner: {lines:?}",
            entry.source_id
        );
    }

    // The account source, with nobody registered: named as refused, with the
    // requirement an operator would have to satisfy. An operator reading a
    // banner that listed only what opened would see a node with nothing to
    // register.
    let account = line_for(&lines, ACCOUNT_SOURCE);
    assert!(
        account.contains("not registered, so refused")
            && account.contains("an account with the venue, opened in the operator's own name"),
        "the banner does not say the source is refused or what it needs: {account}"
    );
    let keyless = line_for(&lines, KEYLESS_SOURCE);
    assert!(
        keyless.contains("keyless; no registration needed"),
        "the banner does not say the keyless source needs nothing: {keyless}"
    );

    // With the owner's record the same line names the person the credential
    // is attributed to, and the credential's variable — never its value.
    let account = line_for(&source_standings(&registered()?)?, ACCOUNT_SOURCE);
    assert!(
        account.contains(&format!("registered by {OWNER}")) && account.contains(ACCOUNT_SLOT),
        "the banner does not name who registered or the slot the credential is read from: \
         {account}"
    );
    Ok(())
}

/// The one banner line naming `source_id`, or a failure saying so.
fn line_for(lines: &[String], source_id: &str) -> String {
    lines
        .iter()
        .find(|line| line.contains(&format!(" {source_id}: ")))
        .unwrap_or_else(|| panic!("no banner line names {source_id}: {lines:?}"))
        .clone()
}

// --- what the node keeps across a restart, and asks again mid-run -------------
//
// Two controls that existed complete and unreachable: `ConnectorFeed::journal_to`
// and `StandingAdmission`, each built, tested and mutation-verified in the crate
// that owns it, and neither called from a composition root. A control nothing
// calls reads as protection and is not. These tests sit at the node's own feed
// seam because that is the only place the wiring can be observed.

/// After the shipped Frankfurter fixture's reference date plus the ECB's
/// publication delay, so a poll at this instant releases the table rather than
/// withholding it as not yet knowable. `start()` is ten months earlier and
/// would release nothing, which would make every count below a zero that meant
/// nothing.
/// A reference hook that accepts every digest, for rigs with no platform.
/// The node passes `Platform::reference_fetch` here; these tests are about
/// the feed's own gates and registrations, not the ledger's.
fn unreferenced(_digest: &qip_market_ingestion::connector::FetchDigest) -> Result<()> {
    Ok(())
}

fn streaming_instant() -> Timestamp {
    Timestamp::parse_rfc3339("2026-08-27T00:00:00Z").expect("a literal instant parses")
}

fn keyless_settings(base_url: &str) -> ConnectorFeedSettings {
    ConnectorFeedSettings {
        source_id: KEYLESS_SOURCE.to_string(),
        base_url: base_url.to_string(),
        seed: 7,
    }
}

fn open_keyless(base_url: &str) -> Result<Feed> {
    Feed::open(
        None,
        Some(&keyless_settings(base_url)),
        None,
        None,
        &PlatformConfig::default().registration_registry()?,
        7,
        Duration::from_secs(1),
        streaming_instant(),
    )
}

/// A store that outlives a "process" the way a mounted volume does, so two
/// feeds opened one after another are two sessions of one stream.
fn journal_store() -> Arc<dyn qip_core::kv::KeyValueStore> {
    Arc::new(qip_storage::kv::MemoryKeyValueStore::new())
}

#[test]
fn a_restarted_node_resumes_the_dedup_window_instead_of_republishing_the_whole_table() -> Result<()>
{
    let server = RateServer::serving(RATE_TABLE);

    // The premise, and the failure the wiring closes: this source serves its
    // whole table on every poll, so a node whose dedup window begins empty
    // republishes all of it as new observations. Two unjournalled restarts,
    // three records each — what every deployed restart did until `journal_to`
    // had a caller in `main`.
    let mut first_run = open_keyless(&server.url)?;
    assert_eq!(
        first_run
            .poll(streaming_instant(), &mut unreferenced)?
            .accepted
            .len(),
        3
    );
    let mut second_run = open_keyless(&server.url)?;
    assert_eq!(
        second_run
            .poll(streaming_instant(), &mut unreferenced)?
            .accepted
            .len(),
        3,
        "without a journal a restart must republish the table; if it does not, the assertion \
         below proves nothing"
    );

    let store = journal_store();
    let mut journalled = open_keyless(&server.url)?;
    assert_eq!(
        journalled.journal_to(store.clone())?,
        Some(0),
        "a first session has no previous checkpoint, so nothing may be resumed"
    );
    assert_eq!(
        journalled
            .poll(streaming_instant(), &mut unreferenced)?
            .accepted
            .len(),
        3
    );
    drop(journalled);

    let mut restarted = open_keyless(&server.url)?;
    let resumed = restarted
        .journal_to(store)?
        .expect("a connector arm keeps a journal");
    assert_eq!(
        resumed, 3,
        "the previous session admitted three records, so three fingerprints must come back"
    );
    assert!(
        restarted
            .poll(streaming_instant(), &mut unreferenced)?
            .accepted
            .is_empty(),
        "the restarted node republished the table it had already published"
    );

    // The other arms have no vendor to redeliver from, so that the assertions
    // above are about the connector and not about a method that answers the
    // same thing for everything.
    let mut synthetic = Feed::synthetic(7, Duration::from_secs(60), streaming_instant());
    assert_eq!(synthetic.journal_to(journal_store())?, None);
    Ok(())
}

#[test]
fn a_licence_that_expires_mid_run_stops_the_next_poll_rather_than_the_next_restart() -> Result<()> {
    let server = RateServer::serving(RATE_TABLE);
    let opened = streaming_instant();
    let expiry = opened.saturating_add(Duration::from_days(3));
    // A licence with an end date. No entry in the shipped catalogue carries
    // one, which is why this arm needs a catalogue of its own: putting an
    // expiry into the evaluation of a real vendor's terms to satisfy a test is
    // the opposite of what that file is for.
    let expiring = vec![CatalogueEntry {
        source_id: KEYLESS_SOURCE,
        expected_class: LicensingClass::Public,
        posture: LicensingPosture::declared(
            SourceLicense::new("terms-with-an-end-date", [Usage::Derive, Usage::Trade])?
                .expiring_at(expiry),
        ),
    }];
    let registrations = PlatformConfig::default().registration_registry()?;
    let mut feed = Feed::connector_admitted_by(
        &expiring,
        &registrations,
        &keyless_settings(&server.url),
        opened,
    )?;

    // Premise: the gate grants for as long as the licence runs, so the refusal
    // below is the expiry doing its work rather than an entry that never
    // admitted anything.
    let granted = feed.poll(
        opened.saturating_add(Duration::from_days(1)),
        &mut unreferenced,
    )?;
    assert_eq!(granted.accepted.len(), 3);
    let served = server.served();
    assert!(served >= 1, "the admitted source opened no socket");

    // Until the standing gate had a caller here, the node would have gone on
    // polling for the remaining four days of a seven-day run, stamping records
    // with a class the terms no longer grant.
    let refusal = feed
        .poll(expiry, &mut unreferenced)
        .expect_err("an expired licence went on feeding the node");
    assert!(
        refusal.message().contains("expired"),
        "the refusal does not say the licence expired: {}",
        refusal.message()
    );
    assert_eq!(
        server.served(),
        served,
        "a refused poll still opened a socket, so the gate ran after the transport rather than \
         before it"
    );

    // And the operator can see the gate is consulted rather than take it on
    // trust: two grants, and the refusal is not counted as one.
    let standing = feed
        .licensing_standing()
        .expect("a connector carries its standing gate");
    assert!(
        standing.contains("2 check(s)"),
        "the banner line does not say how often the gate ran: {standing}"
    );
    Ok(())
}
