//! The document adapter, against a real socket.
//!
//! Every test here binds a listener on loopback and lets the adapter connect to
//! it, for the reason `rest_feed.rs` gives. What these add on top of that file
//! is the part that is specific to documents rather than prices: a filing has a
//! filing time and a period it covers, a vendor revises what it already
//! published, and text carries a licence that a price does not. Each of those
//! is a way to be wrong that no amount of correct HTTP prevents, so each has a
//! test that fails if the adapter gets it wrong.

mod server;

use qip_core::error::Error;
use qip_core::{Context, Duration, Timestamp, dec};
use qip_events::{EventBus, EventLog, Topic};
use qip_financial::intelligence::DataQualityFailure;
use qip_financial::quality::LicensingClass;
use qip_market_ingestion::IngestionService;
use qip_market_ingestion::adapter::{DataAdapter, MacroAdapter, SensedRecord};
use qip_market_ingestion::narrative::{
    NarrativeAdapter, NarrativeFeedConfig, NarrativeSubject, Revision,
};
use qip_observability::Telemetry;
use qip_transport::ClientLimits;
use server::{Action, TestServer, address_with_no_listener};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration as StdDuration;

/// The credential the fixtures use. A literal so a test can assert it never
/// reaches a URL.
const API_KEY: &str = "document-key-91c4";

fn at(text: &str) -> Timestamp {
    Timestamp::parse_rfc3339(text).expect("a fixture timestamp is valid RFC 3339")
}

fn poll_instant() -> Timestamp {
    at("2026-08-24T15:00:00Z")
}

fn subjects() -> Vec<NarrativeSubject> {
    vec![NarrativeSubject::new("ent-northwind", "NWSC")]
}

fn series() -> Vec<String> {
    vec!["US.CPI.YOY".to_string()]
}

/// Limits tight enough that a test trips them in bytes and milliseconds.
fn tight() -> ClientLimits {
    ClientLimits {
        max_body: 8192,
        max_headers: 16,
        connect_timeout: StdDuration::from_millis(500),
        read_timeout: StdDuration::from_millis(200),
        write_timeout: StdDuration::from_millis(500),
        ..ClientLimits::default()
    }
}

/// A fully configured feed pointed at `base`.
///
/// `licensing` is deliberately `None`: the fixtures state their terms per
/// document, which is the shape a text vendor actually has, and it leaves the
/// unset case reachable by a test instead of hidden behind a default.
/// `publication_delay` is zero so that knowability is decided by each
/// document's own instants and not by a constant every assertion would carry.
fn config(base: &str) -> NarrativeFeedConfig {
    NarrativeFeedConfig {
        name: "vendor-documents".into(),
        provider: "a document and macro-release vendor".into(),
        base_url: Some(base.to_string()),
        path: "/v1/narrative".into(),
        api_key: Some(API_KEY.into()),
        api_key_header: "x-api-key".into(),
        licensing: None,
        publication_delay: Duration::ZERO,
        window: Duration::from_mins(30),
        max_records: 100,
        max_document_bytes: 512,
        http: tight(),
    }
}

fn adapter_for(server: &TestServer) -> NarrativeAdapter {
    NarrativeAdapter::new(config(&server.url()), subjects(), series())
        .expect("a fully specified configuration builds")
}

fn adapter_serving(body: &str) -> (TestServer, NarrativeAdapter) {
    let server = TestServer::always(Action::json(200, body));
    let adapter = adapter_for(&server);
    (server, adapter)
}

/// One news item, one filing carrying two figures, one macro release.
///
/// The filing's period ended on 30 June and it was filed on 24 August: the two
/// months between them are what several tests below are about. The news item
/// also carries an `event_time` the decoder must not read.
const FULL_PAYLOAD: &str = r#"{
  "news": [
    {
      "document_id": "wire-2026-08-24-4471",
      "headline": "Northwind Semiconductor warns on fourth-quarter volumes",
      "body": "The company said output at its northern fabrication site would be reduced.",
      "source": "newswire",
      "published_at": "2026-08-24T14:45:00Z",
      "event_time": "2026-08-24T09:12:00Z",
      "entities": [
        {
          "text": "Northwind Semiconductor Corporation",
          "issuer": "NWSC",
          "confidence": 0.94,
          "is_primary": true
        }
      ],
      "sentiment": { "polarity": -0.42, "confidence": 0.81, "novelty": 0.66 },
      "topics": ["guidance", "supply_chain"],
      "licensing": "restricted",
      "revision": { "status": "original" }
    }
  ],
  "filings": [
    {
      "document_id": "0000912057-26-000431",
      "issuer": "NWSC",
      "filed_at": "2026-08-24T14:30:00Z",
      "period_end": "2026-06-30T00:00:00Z",
      "period": "quarter",
      "figures": [
        {
          "metric": "revenue",
          "value": "4310.5",
          "unit": "USD_millions",
          "consensus": "4200.0",
          "prior_value": "3980.0"
        },
        {
          "metric": "operating_margin",
          "value": "0.294",
          "unit": "ratio",
          "consensus": "0.310"
        }
      ],
      "licensing": "licensed",
      "revision": { "status": "original" }
    }
  ],
  "macro": [
    {
      "document_id": "stat-cpi-2026-07",
      "series_id": "US.CPI.YOY",
      "region": "US",
      "value": 2.9,
      "unit": "percent",
      "reference_date": "2026-07-31T00:00:00Z",
      "released_at": "2026-08-24T12:30:00Z",
      "consensus": 3.1,
      "previous": 3.0,
      "licensing": "public",
      "revision": { "status": "original" }
    }
  ]
}"#;

fn news_of(records: &[SensedRecord]) -> &qip_financial::intelligence::NewsItem {
    records
        .iter()
        .find_map(|r| match r {
            SensedRecord::News(item) => Some(&**item),
            _ => None,
        })
        .expect("the payload carries a news item")
}

fn fundamentals_of(
    records: &[SensedRecord],
) -> Vec<&qip_financial::intelligence::FundamentalUpdate> {
    records
        .iter()
        .filter_map(|r| match r {
            SensedRecord::Fundamental(update) => Some(&**update),
            _ => None,
        })
        .collect()
}

fn macro_of(records: &[SensedRecord]) -> &qip_financial::intelligence::MacroObservation {
    records
        .iter()
        .find_map(|r| match r {
            SensedRecord::Macro(observation) => Some(&**observation),
            _ => None,
        })
        .expect("the payload carries a macro release")
}

// --- what arrives -----------------------------------------------------------

#[test]
fn a_configured_adapter_fetches_over_a_real_socket_and_decodes_news_a_filing_and_a_macro_release() {
    let (server, mut adapter) = adapter_serving(FULL_PAYLOAD);

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");

    assert_eq!(
        server.served(),
        1,
        "the premise of this test is that a request crossed a socket; it did not"
    );
    assert_eq!(
        records.len(),
        4,
        "a news item, two figures from one filing and a macro release: {records:?}"
    );
    assert_eq!(adapter.stats().fetches, 1);
    assert_eq!(adapter.stats().emitted, 4);
    assert_eq!(adapter.stats().withheld, 0);
    assert_eq!(adapter.stats().revisions, 0, "every fixture is an original");

    let topics: Vec<Topic> = records.iter().map(SensedRecord::topic).collect();
    assert!(topics.contains(&Topic::NewsReceived));
    assert!(topics.contains(&Topic::FundamentalUpdated));
    assert!(topics.contains(&Topic::MacroUpdated));

    let item = news_of(&records);
    assert_eq!(item.item_id, "wire-2026-08-24-4471");
    assert_eq!(
        item.headline,
        "Northwind Semiconductor warns on fourth-quarter volumes"
    );
    assert!(
        item.body.contains("northern fabrication site"),
        "the body arrives whole: {}",
        item.body
    );
    assert_eq!(
        item.entities[0].entity_id.as_deref(),
        Some("ent-northwind"),
        "the vendor's issuer key resolved through the configured subject map"
    );
    let news_record = records
        .iter()
        .find(|r| matches!(r, SensedRecord::News(_)))
        .expect("the news record is there");
    assert!(
        news_record.validate().is_empty(),
        "the decoded item is publishable"
    );

    let figures = fundamentals_of(&records);
    assert_eq!(figures.len(), 2, "one record per reported figure");
    let revenue = figures
        .iter()
        .find(|f| f.metric == "revenue")
        .expect("the revenue figure decoded");
    assert_eq!(revenue.entity_id, "ent-northwind");
    assert_eq!(revenue.value, dec!("4310.5"));
    assert_eq!(revenue.unit, "USD_millions");
    assert_eq!(revenue.consensus, Some(dec!("4200.0")));
    assert_eq!(revenue.prior_value, Some(dec!("3980.0")));

    let observation = macro_of(&records);
    assert_eq!(observation.series_id, "US.CPI.YOY");
    assert_eq!(observation.region, "US");
    assert_eq!(observation.consensus, Some(3.1));
    assert!(
        !observation.is_revision,
        "the fixture declares itself an original"
    );
}

#[test]
fn the_records_come_back_in_event_order_whatever_order_the_vendor_listed_them_in() {
    let (_server, mut adapter) = adapter_serving(FULL_PAYLOAD);

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");

    let times: Vec<i64> = records.iter().map(|r| r.occurred_at().as_nanos()).collect();
    let mut sorted = times.clone();
    sorted.sort_unstable();
    assert_eq!(
        times, sorted,
        "a consumer that assumes a monotone stream must get one"
    );
}

// --- which of a document's two instants is taken ----------------------------

#[test]
fn a_news_item_is_stamped_with_its_publication_time_and_not_with_the_event_it_describes() {
    // The fixture's `event_time` is 09:12 and its `published_at` is 14:45. A
    // decoder that took the earlier one would make the story look actionable
    // five hours before anyone could read it.
    let (_server, mut adapter) = adapter_serving(FULL_PAYLOAD);

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");
    let item = news_of(&records);

    assert_eq!(
        item.published_at,
        at("2026-08-24T14:45:00Z"),
        "the publication instant is the only one a NewsItem carries"
    );
    assert_eq!(
        item.provenance.event_time,
        at("2026-08-24T14:45:00Z"),
        "the provenance records the instant the document entered the world"
    );
    assert_ne!(
        item.published_at,
        at("2026-08-24T09:12:00Z"),
        "the event the story describes must not be read as its publication"
    );
}

#[test]
fn a_filing_is_stamped_with_its_filing_time_rather_than_the_end_of_the_period_it_covers() {
    let (_server, mut adapter) = adapter_serving(FULL_PAYLOAD);

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");
    let figures = fundamentals_of(&records);

    for figure in &figures {
        assert_eq!(
            figure.period_end,
            at("2026-06-30T00:00:00Z"),
            "the period the figure covers stays on the record: it is the valid time"
        );
        assert_eq!(
            figure.provenance.event_time,
            at("2026-08-24T14:30:00Z"),
            "the instant that decides knowability is the filing, and it is recorded so an audit \
             can subtract the two"
        );
        assert_eq!(
            figure.provenance.ingestion_time,
            poll_instant(),
            "the caller's clock, not the wall clock, so a replay reproduces this record"
        );
        assert_eq!(
            figure.provenance.upstream_id.as_deref(),
            Some("0000912057-26-000431"),
            "the filing's own identity, so a reconciliation has something to join on"
        );
    }
    assert!(
        figures[0].provenance.event_time > figures[0].period_end,
        "the premise of this test is that the two instants differ; they do not"
    );
}

#[test]
fn a_filing_whose_period_ended_months_ago_is_withheld_until_the_clock_reaches_when_it_was_filed() {
    // The one that matters. The quarter ended on 30 June; the filing is dated
    // 14:30 on 24 August. Polled at 14:00, an adapter keyed on the period end
    // would hand over a figure nobody had yet seen.
    let (_server, mut adapter) = adapter_serving(FULL_PAYLOAD);

    let early = adapter
        .poll(at("2026-08-24T14:00:00Z"))
        .expect("the fetch succeeds");

    assert!(
        fundamentals_of(&early).is_empty(),
        "a filing must not be knowable before it was filed: {early:?}"
    );
    assert_eq!(
        adapter.stats().withheld,
        3,
        "both figures from the not-yet-filed document are withheld rather than dropped, along          with the story published at 14:45; only the 12:30 macro print was knowable"
    );

    let later = adapter.poll(poll_instant()).expect("the fetch succeeds");
    assert_eq!(
        fundamentals_of(&later).len(),
        2,
        "the next poll whose window covers the filing hands both figures over"
    );
}

#[test]
fn a_macro_release_is_knowable_when_it_was_released_and_not_on_the_date_it_describes() {
    let (_server, mut adapter) = adapter_serving(FULL_PAYLOAD);

    let early = adapter
        .poll(at("2026-08-24T12:00:00Z"))
        .expect("the fetch succeeds");
    assert!(
        !early.iter().any(|r| matches!(r, SensedRecord::Macro(_))),
        "a July print released at 12:30 is not knowable at 12:00: {early:?}"
    );

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");
    let observation = macro_of(&records);
    assert_eq!(
        observation.reference_date,
        at("2026-07-31T00:00:00Z"),
        "the month the statistic describes stays on the record"
    );
    assert_eq!(
        observation.provenance.event_time,
        at("2026-08-24T12:30:00Z"),
        "the release instant is what gated the record and what is recorded"
    );
}

// --- restatement ------------------------------------------------------------

const REVISED_PAYLOAD: &str = r#"{
  "news": [
    {
      "document_id": "wire-2026-08-24-4482",
      "headline": "CORRECTED: Northwind Semiconductor volume guidance",
      "body": "Corrects the figure given in the earlier story.",
      "source": "newswire",
      "published_at": "2026-08-24T14:50:00Z",
      "entities": [],
      "topics": ["guidance"],
      "licensing": "restricted",
      "revision": { "status": "revision", "revises": "wire-2026-08-24-4471" }
    }
  ],
  "filings": [
    {
      "document_id": "0000912057-26-000502",
      "issuer": "NWSC",
      "filed_at": "2026-08-24T14:35:00Z",
      "period_end": "2026-06-30T00:00:00Z",
      "period": "quarter",
      "figures": [
        { "metric": "revenue", "value": "4180.0", "unit": "USD_millions" }
      ],
      "licensing": "licensed",
      "revision": { "status": "revision", "revises": "0000912057-26-000431" }
    }
  ],
  "macro": [
    {
      "document_id": "stat-cpi-2026-07-r1",
      "series_id": "US.CPI.YOY",
      "region": "US",
      "value": 3.0,
      "unit": "percent",
      "reference_date": "2026-07-31T00:00:00Z",
      "released_at": "2026-08-24T12:45:00Z",
      "licensing": "public",
      "revision": { "status": "revision", "revises": "stat-cpi-2026-07" }
    }
  ]
}"#;

#[test]
fn a_restated_filing_arrives_marked_a_restatement_naming_the_filing_it_restates() {
    let (_server, mut adapter) = adapter_serving(REVISED_PAYLOAD);

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");
    let figures = fundamentals_of(&records);

    assert_eq!(figures.len(), 1);
    assert!(
        figures[0].is_restatement,
        "a revised figure recorded as an original is the leakage LeakageAudit exists to catch"
    );
    assert_eq!(
        figures[0].provenance.derived_from,
        vec!["0000912057-26-000431".to_string()],
        "what it restates has to be nameable, or it cannot be reconciled against what it replaced"
    );
    assert_ne!(
        figures[0].provenance.upstream_id.as_deref(),
        Some("0000912057-26-000431"),
        "the restatement keeps its own identity"
    );
    assert_eq!(adapter.stats().revisions, 3, "all three fixtures revise");
}

#[test]
fn a_revised_macro_print_arrives_marked_a_revision_naming_the_print_it_revises() {
    let (_server, mut adapter) = adapter_serving(REVISED_PAYLOAD);

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");
    let observation = macro_of(&records);

    assert!(
        observation.is_revision,
        "a revised series read as first-published history is a backtest reading the future"
    );
    assert_eq!(
        observation.provenance.derived_from,
        vec!["stat-cpi-2026-07".to_string()]
    );
    assert_eq!(
        observation.reference_date,
        at("2026-07-31T00:00:00Z"),
        "a revision still describes the same month"
    );
    assert_eq!(
        observation.provenance.event_time,
        at("2026-08-24T12:45:00Z"),
        "and became knowable when the revision was released, not when the original was"
    );
}

#[test]
fn a_corrected_news_item_carries_the_correction_in_its_topics_and_in_its_lineage() {
    let (_server, mut adapter) = adapter_serving(REVISED_PAYLOAD);

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");
    let item = news_of(&records);

    assert!(
        item.topics.contains(&"revision".to_string()),
        "NewsItem has no restatement flag, so the tag is where this can be said: {:?}",
        item.topics
    );
    assert_eq!(
        item.topics.first().map(String::as_str),
        Some("guidance"),
        "appended and not prepended: the catalyst layer classifies on the first topic, and a \
         correction to a guidance story is still a guidance story"
    );
    assert_eq!(
        item.provenance.derived_from,
        vec!["wire-2026-08-24-4471".to_string()]
    );
}

#[test]
fn a_document_that_does_not_say_whether_it_is_a_revision_is_refused_rather_than_assumed_original() {
    let (_server, mut adapter) = adapter_serving(
        r#"{"macro":[{"document_id":"stat-cpi-2026-07","series_id":"US.CPI.YOY","region":"US",
            "value":2.9,"unit":"percent","reference_date":"2026-07-31T00:00:00Z",
            "released_at":"2026-08-24T12:30:00Z","licensing":"public"}]}"#,
    );

    let error = adapter
        .poll(poll_instant())
        .expect_err("a document with no revision status was accepted");

    assert_eq!(error.code(), "schema", "got {error:?}");
    assert!(
        error.message().contains("revision status"),
        "the refusal must name what was missing: {error}"
    );
    assert_eq!(
        adapter.stats().emitted,
        0,
        "nothing may be published from a document whose history is unknown"
    );
}

#[test]
fn a_revision_that_reuses_the_originals_id_is_refused_because_the_bus_would_drop_it_as_a_redelivery()
 {
    let (_server, mut adapter) = adapter_serving(
        r#"{"macro":[{"document_id":"stat-cpi-2026-07","series_id":"US.CPI.YOY","region":"US",
            "value":3.0,"unit":"percent","reference_date":"2026-07-31T00:00:00Z",
            "released_at":"2026-08-24T12:45:00Z","licensing":"public",
            "revision":{"status":"revision","revises":"stat-cpi-2026-07"}}]}"#,
    );

    let error = adapter
        .poll(poll_instant())
        .expect_err("a self-revising document was accepted");

    assert_eq!(error.code(), "schema", "got {error:?}");
    assert!(
        error.message().contains("redelivery"),
        "the refusal must say why the correction would vanish: {error}"
    );
}

#[test]
fn a_revision_naming_nothing_is_refused_because_it_cannot_be_reconciled() {
    let (_server, mut adapter) = adapter_serving(
        r#"{"macro":[{"document_id":"stat-cpi-2026-07-r1","series_id":"US.CPI.YOY","region":"US",
            "value":3.0,"unit":"percent","reference_date":"2026-07-31T00:00:00Z",
            "released_at":"2026-08-24T12:45:00Z","licensing":"public",
            "revision":{"status":"revision","revises":"  "}}]}"#,
    );

    let error = adapter
        .poll(poll_instant())
        .expect_err("a revision with no antecedent was accepted");
    assert_eq!(error.code(), "schema", "got {error:?}");
}

#[test]
fn the_revision_type_offers_no_unknown_variant_to_be_defaulted_into() {
    // A type-level assertion, not a behavioural one: the reason a document with
    // no stated status is refused at decode time is that there is no value to
    // decode it into.
    assert!(!Revision::Original.is_revision());
    assert_eq!(Revision::Original.supersedes(), None);
    let revision = Revision::Revises("doc-1".into());
    assert!(revision.is_revision());
    assert_eq!(revision.supersedes(), Some("doc-1"));
}

// --- licensing --------------------------------------------------------------

#[test]
fn every_decoded_record_carries_the_licensing_class_its_document_stated() {
    let (_server, mut adapter) = adapter_serving(FULL_PAYLOAD);

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");

    let item = news_of(&records);
    assert_eq!(
        item.provenance.licensing,
        LicensingClass::Restricted,
        "the class the document stated, not the config's"
    );
    assert!(
        !item.provenance.licensing.allows_raw_display(),
        "restricted text may drive features and may not be shown, and the record says so"
    );
    assert!(
        !item.body.is_empty(),
        "this adapter labels rather than redacting: the text is still here for the derived use"
    );
    assert_eq!(
        fundamentals_of(&records)[0].provenance.licensing,
        LicensingClass::Licensed
    );
    assert_eq!(
        macro_of(&records).provenance.licensing,
        LicensingClass::Public
    );
}

#[test]
fn a_document_with_no_licensing_class_and_no_feed_default_is_refused_rather_than_defaulted() {
    let (_server, mut adapter) = adapter_serving(
        r#"{"news":[{"document_id":"wire-1","headline":"Northwind names a chief executive",
            "body":"","source":"newswire","published_at":"2026-08-24T14:45:00Z",
            "revision":{"status":"original"}}]}"#,
    );
    assert!(
        adapter.config().licensing.is_none(),
        "the premise: this feed configures no default class"
    );

    let error = adapter
        .poll(poll_instant())
        .expect_err("a document with no licensing terms was accepted");

    assert_eq!(error.code(), "denied", "got {error:?}");
    assert!(
        error.message().contains("Internal"),
        "the refusal must say what the silent default would have granted: {error}"
    );
    assert_eq!(
        LicensingClass::default(),
        LicensingClass::Internal,
        "the premise of the refusal: the default class permits raw display"
    );
    assert!(
        LicensingClass::default().allows_raw_display(),
        "which is why an unset class must not be allowed to become the default"
    );
}

#[test]
fn a_feed_wide_licensing_class_covers_a_document_that_states_none() {
    let server = TestServer::always(Action::json(
        200,
        r#"{"news":[{"document_id":"wire-1","headline":"Northwind names a chief executive",
            "body":"","source":"newswire","published_at":"2026-08-24T14:45:00Z",
            "revision":{"status":"original"}}]}"#,
    ));
    let mut settings = config(&server.url());
    settings.licensing = Some(LicensingClass::Licensed);
    let mut adapter =
        NarrativeAdapter::new(settings, subjects(), series()).expect("the configuration builds");

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");

    assert_eq!(records.len(), 1);
    assert_eq!(
        news_of(&records).provenance.licensing,
        LicensingClass::Licensed,
        "a vendor whose terms are contractual rather than per document is served by the default"
    );
}

#[test]
fn a_licensing_class_this_decoder_cannot_name_is_refused_rather_than_mapped_to_the_nearest() {
    let (_server, mut adapter) = adapter_serving(
        r#"{"news":[{"document_id":"wire-1","headline":"Northwind names a chief executive",
            "body":"","source":"newswire","published_at":"2026-08-24T14:45:00Z",
            "licensing":"press-embargo","revision":{"status":"original"}}]}"#,
    );

    let error = adapter
        .poll(poll_instant())
        .expect_err("an unrecognised licensing class was accepted");

    assert_eq!(error.code(), "schema", "got {error:?}");
    assert!(
        error.message().contains("restricted"),
        "the refusal must list the classes it does accept: {error}"
    );
}

#[test]
fn an_unconfigured_feed_describes_itself_with_the_class_that_forbids_raw_display() {
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let adapter = adapter_for(&server);

    let descriptor = adapter.descriptor();

    assert_eq!(
        descriptor.licensing,
        LicensingClass::Restricted,
        "with no feed-wide class, the source cannot promise that its text may be displayed"
    );
    assert!(
        descriptor.is_production_grade(),
        "restricted still admits a capital decision; it is display it withholds"
    );
    assert_eq!(
        descriptor.topics,
        vec![
            Topic::NewsReceived,
            Topic::FundamentalUpdated,
            Topic::MacroUpdated
        ]
    );
}

// --- bounds -----------------------------------------------------------------

#[test]
fn a_news_document_larger_than_the_per_document_cap_is_refused_rather_than_truncated() {
    let body = "a".repeat(1200);
    let payload = format!(
        r#"{{"news":[{{"document_id":"wire-1","headline":"Northwind names a chief executive",
            "body":"{body}","source":"newswire","published_at":"2026-08-24T14:45:00Z",
            "licensing":"public","revision":{{"status":"original"}}}}]}}"#
    );
    let (_server, mut adapter) = adapter_serving(&payload);
    assert!(
        payload.len() < adapter.config().http.max_body,
        "the premise: the response as a whole is within the transport limit"
    );
    assert_eq!(
        adapter.config().max_document_bytes,
        512,
        "the premise: the single document exceeds the per-document cap"
    );

    let error = adapter
        .poll(poll_instant())
        .expect_err("an oversized document was accepted");

    assert_eq!(error.code(), "guard", "got {error:?}");
    assert!(
        error.message().contains("512"),
        "the refusal must say what the cap was: {error}"
    );
    assert!(
        error.message().contains("truncated"),
        "and why it refuses instead of shortening the document: {error}"
    );
}

#[test]
fn a_response_larger_than_the_transport_limit_is_refused_before_it_is_buffered() {
    let server = TestServer::always(Action::Oversized { bytes: 64 * 1024 });
    let mut adapter = adapter_for(&server);
    assert_eq!(
        adapter.config().http.max_body,
        8192,
        "the premise: the fixture exceeds the cap"
    );

    let error: Error = adapter
        .poll(poll_instant())
        .expect_err("a 64 kB body was accepted against an 8 kB limit");

    assert!(matches!(error, Error::Guard(_)), "got {error:?}");
    assert!(error.message().contains("8192"), "{error}");
}

#[test]
fn more_records_than_the_cap_are_refused_counting_one_per_figure_rather_than_one_per_filing() {
    let figures: Vec<String> = (0..4)
        .map(|i| format!(r#"{{"metric":"line_item_{i}","value":"1.0","unit":"USD_millions"}}"#))
        .collect();
    let payload = format!(
        r#"{{"filings":[{{"document_id":"filing-1","issuer":"NWSC",
            "filed_at":"2026-08-24T14:30:00Z","period_end":"2026-06-30T00:00:00Z",
            "period":"quarter","licensing":"licensed","revision":{{"status":"original"}},
            "figures":[{}]}}]}}"#,
        figures.join(",")
    );
    let server = TestServer::always(Action::json(200, payload));
    let mut settings = config(&server.url());
    settings.max_records = 3;
    let mut adapter =
        NarrativeAdapter::new(settings, subjects(), series()).expect("the configuration builds");

    let error = adapter
        .poll(poll_instant())
        .expect_err("one filing expanding to four records passed a cap of three");

    assert_eq!(error.code(), "guard", "got {error:?}");
    assert!(
        error.message().contains("4 records and the cap is 3"),
        "the cap counts what the response expands into: {error}"
    );
}

#[test]
fn a_filing_reporting_no_figures_at_all_is_refused_as_a_decode_that_lost_its_content() {
    let (_server, mut adapter) = adapter_serving(
        r#"{"filings":[{"document_id":"filing-1","issuer":"NWSC",
            "filed_at":"2026-08-24T14:30:00Z","period_end":"2026-06-30T00:00:00Z",
            "period":"quarter","licensing":"licensed","revision":{"status":"original"},
            "figures":[]}]}"#,
    );

    let error = adapter
        .poll(poll_instant())
        .expect_err("an empty filing was accepted");
    assert_eq!(error.code(), "schema", "got {error:?}");
}

// --- a peer having a bad day ------------------------------------------------

#[test]
fn a_peer_that_dies_part_way_through_its_own_body_is_a_close_and_not_a_short_document() {
    let server = TestServer::always(Action::Truncated {
        declared: 4096,
        written: 64,
    });
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("half a body was accepted as a document");

    assert_eq!(
        error.code(),
        "io",
        "a peer that stopped mid-body is a connection that failed, not a schema that is wrong: \
         {error:?}"
    );
    assert_eq!(
        adapter.stats().emitted,
        0,
        "no part of a half-received document may be published"
    );
}

#[test]
fn a_peer_that_accepts_the_connection_and_says_nothing_is_refused_within_the_timeout() {
    let server = TestServer::always(Action::Silent(StdDuration::from_secs(3)));
    let mut adapter = adapter_for(&server);

    let started = std::time::Instant::now();
    let error = adapter
        .poll(poll_instant())
        .expect_err("a silent peer was waited on indefinitely");

    assert_eq!(error.code(), "timeout", "got {error:?}");
    assert!(
        started.elapsed() < StdDuration::from_secs(2),
        "the wait must be bounded by the configured read timeout, not by the peer"
    );
}

#[test]
fn an_unreachable_vendor_is_refused_rather_than_waited_on() {
    let mut settings = config(&address_with_no_listener());
    settings.http.connect_timeout = StdDuration::from_millis(500);
    let mut adapter =
        NarrativeAdapter::new(settings, subjects(), series()).expect("the configuration builds");

    let started = std::time::Instant::now();
    let error = adapter
        .poll(poll_instant())
        .expect_err("a fetch from an address with no listener produced records");

    assert!(
        matches!(error, Error::Io(_) | Error::Timeout(_)),
        "an address nothing answers on is a connection failure: {error:?}"
    );
    assert!(
        started.elapsed() < StdDuration::from_secs(2),
        "the attempt must be bounded"
    );
    assert_eq!(adapter.stats().emitted, 0);
}

#[test]
fn a_body_that_is_not_json_is_refused_with_an_error_that_names_the_feed() {
    let (_server, mut adapter) = adapter_serving("{\"news\": [ {\"document_id\"");

    let error = adapter
        .poll(poll_instant())
        .expect_err("a truncated JSON document was accepted");

    assert_eq!(error.code(), "schema", "got {error:?}");
    assert!(
        error.message().contains("vendor-documents"),
        "the refusal must name which feed sent it: {error}"
    );
}

#[test]
fn a_vendor_that_refuses_on_legal_grounds_says_so_rather_than_looking_like_an_outage() {
    let server = TestServer::always(Action::json(
        451,
        "{\"error\":\"licence does not cover EU\"}",
    ));
    let mut adapter = adapter_for(&server);

    let error = adapter
        .poll(poll_instant())
        .expect_err("HTTP 451 was read as a success");

    assert_eq!(
        error.code(),
        "denied",
        "a licence refusal is not something to retry: {error:?}"
    );
    assert!(error.message().contains("licence"), "{error}");
}

// --- what the vendor may not decide -----------------------------------------

#[test]
fn a_filing_for_an_issuer_this_deployment_has_not_mapped_is_refused_not_given_an_invented_id() {
    let (_server, mut adapter) = adapter_serving(
        r#"{"filings":[{"document_id":"filing-1","issuer":"VNTG",
            "filed_at":"2026-08-24T14:30:00Z","period_end":"2026-06-30T00:00:00Z",
            "period":"quarter","licensing":"licensed","revision":{"status":"original"},
            "figures":[{"metric":"revenue","value":"1.0","unit":"USD_millions"}]}]}"#,
    );

    let error = adapter
        .poll(poll_instant())
        .expect_err("a filing for an unmapped issuer was accepted");

    assert_eq!(error.code(), "not_found", "got {error:?}");
    assert!(
        error.message().contains("NWSC"),
        "the refusal must say what the deployment does map: {error}"
    );
}

#[test]
fn a_macro_series_this_deployment_does_not_cover_is_refused() {
    let (_server, mut adapter) = adapter_serving(
        r#"{"macro":[{"document_id":"stat-1","series_id":"US.PPI.MOM","region":"US","value":0.4,
            "unit":"percent","reference_date":"2026-07-31T00:00:00Z",
            "released_at":"2026-08-24T12:30:00Z","licensing":"public",
            "revision":{"status":"original"}}]}"#,
    );

    let error = adapter
        .poll(poll_instant())
        .expect_err("an unconfigured series was accepted");

    assert_eq!(error.code(), "not_found", "got {error:?}");
    assert_eq!(
        adapter.series(),
        vec!["US.CPI.YOY".to_string()],
        "the covered set is what a deployment configured, not what a vendor sent"
    );
}

#[test]
fn a_news_source_this_decoder_cannot_name_is_refused_rather_than_weighted_as_a_newswire() {
    let (_server, mut adapter) = adapter_serving(
        r#"{"news":[{"document_id":"wire-1","headline":"Northwind names a chief executive",
            "body":"","source":"anonymous_tip","published_at":"2026-08-24T14:45:00Z",
            "licensing":"public","revision":{"status":"original"}}]}"#,
    );

    let error = adapter
        .poll(poll_instant())
        .expect_err("an unknown source was accepted");

    assert_eq!(error.code(), "schema", "got {error:?}");
    assert!(error.message().contains("newswire"), "{error}");
}

#[test]
fn a_fiscal_period_this_decoder_cannot_name_is_refused_rather_than_read_as_a_quarter() {
    let (_server, mut adapter) = adapter_serving(
        r#"{"filings":[{"document_id":"filing-1","issuer":"NWSC",
            "filed_at":"2026-08-24T14:30:00Z","period_end":"2026-06-30T00:00:00Z",
            "period":"four_weeks","licensing":"licensed","revision":{"status":"original"},
            "figures":[{"metric":"revenue","value":"1.0","unit":"USD_millions"}]}]}"#,
    );

    let error = adapter
        .poll(poll_instant())
        .expect_err("an unknown fiscal period was accepted");
    assert_eq!(error.code(), "schema", "got {error:?}");
}

#[test]
fn an_unresolved_issuer_mention_is_counted_rather_than_given_an_invented_entity_id() {
    let (_server, mut adapter) = adapter_serving(
        r#"{"news":[{"document_id":"wire-1","headline":"Two chip makers extend a supply deal",
            "body":"","source":"newswire","published_at":"2026-08-24T14:45:00Z",
            "licensing":"public","revision":{"status":"original"},
            "entities":[{"text":"Vantage Devices","issuer":"VNTG","confidence":0.7,
                         "is_primary":true}]}]}"#,
    );

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");
    let item = news_of(&records);

    assert_eq!(
        item.entities[0].entity_id, None,
        "an unmapped issuer leaves the mention unresolved; identity resolution runs downstream"
    );
    assert_eq!(
        item.entities[0].text, "Vantage Devices",
        "the surface form survives so resolution has something to work with"
    );
    assert_eq!(
        adapter.stats().unresolved_mentions,
        1,
        "the symptom of a mistyped issuer key is silence, so it is counted"
    );
}

#[test]
fn a_news_item_the_vendor_did_not_score_records_no_confidence_rather_than_a_neutral_reading() {
    let (_server, mut adapter) = adapter_serving(
        r#"{"news":[{"document_id":"wire-1","headline":"Northwind names a chief executive",
            "body":"","source":"newswire","published_at":"2026-08-24T14:45:00Z",
            "licensing":"public","revision":{"status":"original"}}]}"#,
    );

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");
    let sentiment = news_of(&records).sentiment;

    // Compared against zero by magnitude rather than by `==`: the value is an
    // exact literal today, and a test that would break if it were ever computed
    // is testing the arithmetic rather than the intent.
    assert!(
        sentiment.confidence.abs() < 1e-12,
        "a reading nobody made is not a confident neutral one: {}",
        sentiment.confidence
    );
    assert!(sentiment.polarity.abs() < 1e-12);
    assert!(!sentiment.is_material());
}

#[test]
fn a_sentiment_the_vendor_got_wrong_reaches_the_validation_gate_instead_of_being_dropped_here() {
    // Polarity outside [-1, 1]. The adapter deliberately does not correct it: a
    // record fixed inside an adapter is a vendor fault nobody can see.
    let (_server, mut adapter) = adapter_serving(
        r#"{"news":[{"document_id":"wire-1","headline":"Northwind names a chief executive",
            "body":"","source":"newswire","published_at":"2026-08-24T14:45:00Z",
            "licensing":"public","revision":{"status":"original"},
            "sentiment":{"polarity":4.2,"confidence":0.9,"novelty":0.5}}]}"#,
    );

    let records = adapter.poll(poll_instant()).expect("the fetch succeeds");
    assert_eq!(records.len(), 1, "the adapter hands the bad item on");
    assert!(
        !records[0].validate().is_empty(),
        "the premise: this sentiment does not survive validation"
    );

    let now = poll_instant();
    let (context, _clock) = Context::deterministic(now, 1);
    let log = Rc::new(RefCell::new(EventLog::in_memory()));
    let mut bus = EventBus::new().with_log(log.clone());
    let mut service = IngestionService::new(Telemetry::silent());
    let published = service
        .publish_records(&context, &mut bus, "vendor-documents", &records)
        .expect("publication succeeds");
    bus.drain(&context).expect("the bus drains");

    assert_eq!(published, 0, "the bad item must not publish as news");
    let log = log.borrow();
    assert_eq!(log.by_topic(Topic::NewsReceived).len(), 0);
    let failures = log.by_topic(Topic::DataQualityFailed);
    assert_eq!(failures.len(), 1, "the vendor's error must be visible");
    let failure: DataQualityFailure = failures[0]
        .decode::<DataQualityFailure>()
        .expect("the failure decodes")
        .body;
    assert_eq!(failure.source, "vendor-documents");
    assert!(failure.rejected);
}

// --- the request --------------------------------------------------------------

#[test]
fn the_request_names_the_window_over_publication_time_and_the_coverage_it_was_configured_with() {
    let (server, mut adapter) = adapter_serving(FULL_PAYLOAD);

    adapter.poll(poll_instant()).expect("the fetch succeeds");

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].method, "GET",
        "a fetch reads and changes nothing"
    );
    let target = &requests[0].target;
    assert!(
        target.starts_with("/v1/narrative?"),
        "unexpected target {target}"
    );
    assert!(
        target.contains("published_until=2026-08-24T15:00:00"),
        "the window's end is the caller's clock: {target}"
    );
    assert!(
        target.contains("published_since=2026-08-24T14:30:00"),
        "the window's start is `until` less the configured window: {target}"
    );
    assert!(
        target.contains("published_since") && !target.contains("period_since"),
        "the parameter names say the window is over publication time, which is the only place \
         this adapter can tell a vendor not to filter by the period a filing covers: {target}"
    );
    assert!(target.contains("issuers=NWSC"), "{target}");
    assert!(target.contains("series=US.CPI.YOY"), "{target}");
}

#[test]
fn the_credential_travels_in_a_header_and_never_in_the_url() {
    let (server, mut adapter) = adapter_serving(FULL_PAYLOAD);

    adapter.poll(poll_instant()).expect("the fetch succeeds");

    let requests = server.requests();
    assert_eq!(
        requests[0].headers.get("x-api-key").map(String::as_str),
        Some(API_KEY),
        "the credential must reach the vendor"
    );
    assert!(
        !requests[0].target.contains(API_KEY),
        "a URL is written to every access log on the path: {}",
        requests[0].target
    );
    let redacted = format!("{:?}", adapter.config());
    assert!(
        !redacted.contains(API_KEY),
        "the config's Debug must not print the credential: {redacted}"
    );
    assert!(redacted.contains("<redacted>"), "{redacted}");
}

// --- an adapter with nothing behind it --------------------------------------

#[test]
fn an_unconfigured_adapter_names_every_missing_piece_and_opens_no_connection() {
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let mut adapter = NarrativeAdapter::new(NarrativeFeedConfig::default(), Vec::new(), Vec::new())
        .expect("an adapter that cannot fetch still has to exist in order to say so");

    assert!(!adapter.is_available());
    let missing = adapter.missing_configuration();
    assert_eq!(
        missing.len(),
        3,
        "three separate things are missing: {missing:?}"
    );
    let joined = missing.join(" | ");
    assert!(joined.contains("no endpoint"), "{joined}");
    assert!(joined.contains("no credential"), "{joined}");
    assert!(joined.contains("no coverage"), "{joined}");

    let requirement = adapter
        .descriptor()
        .production_requirement
        .expect("the descriptor must carry the requirement");
    assert!(requirement.contains("base_url"), "{requirement}");
    assert!(requirement.contains("api_key"), "{requirement}");
    assert!(requirement.contains("entity id"), "{requirement}");
    assert!(
        requirement.contains("redistribution licence"),
        "a text feed's requirement has to name the licence: {requirement}"
    );

    let error = adapter
        .poll(poll_instant())
        .expect_err("an unconfigured adapter produced records");
    assert_eq!(error.code(), "unavailable", "got {error:?}");
    assert!(
        error
            .message()
            .contains("will not substitute generated documents"),
        "the refusal must say that no data is deliberate: {error}"
    );

    let error = adapter
        .start(poll_instant())
        .expect_err("an unconfigured adapter started");
    assert_eq!(
        error.code(),
        "unavailable",
        "a missing credential should fail the rollout, not the first poll an hour later"
    );
    assert_eq!(
        server.served(),
        0,
        "an unconfigured adapter must not open a connection at all"
    );
}

#[test]
fn an_endpoint_without_a_credential_is_still_unavailable_and_still_opens_nothing() {
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let mut settings = config(&server.url());
    settings.api_key = None;
    let mut adapter =
        NarrativeAdapter::new(settings, subjects(), series()).expect("the configuration builds");

    assert!(!adapter.is_available());
    assert_eq!(adapter.missing_configuration().len(), 1);

    let error = adapter
        .poll(poll_instant())
        .expect_err("a keyless fetch was attempted");
    assert_eq!(error.code(), "unavailable", "got {error:?}");
    assert_eq!(
        server.served(),
        0,
        "an adapter with no credential must not open a connection at all"
    );
}

#[test]
fn a_configured_adapter_still_states_what_production_has_to_add() {
    let (_server, adapter) = adapter_serving(FULL_PAYLOAD);

    let requirement = adapter
        .descriptor()
        .production_requirement
        .expect("a configured adapter is not by itself a production feed");

    assert!(
        adapter.is_available(),
        "the premise: nothing is missing from the configuration"
    );
    assert!(requirement.contains("TLS"), "{requirement}");
    assert!(
        requirement.contains("as published rather than as currently amended"),
        "the silent-revision limit is a promise this adapter cannot make and has to name: \
         {requirement}"
    );
    assert!(requirement.contains("dissemination delay"), "{requirement}");
}

#[test]
fn an_https_endpoint_is_refused_at_configuration_time_rather_than_downgraded() {
    let mut settings = config("https://documents.example.com");
    settings.base_url = Some("https://documents.example.com".into());

    let error = NarrativeAdapter::new(settings, subjects(), series())
        .expect_err("an https endpoint was accepted by a client with no TLS stack");

    assert_eq!(error.code(), "invalid", "got {error:?}");
}

#[test]
fn two_subjects_claiming_one_issuer_key_are_refused() {
    let error = NarrativeAdapter::new(
        config("http://127.0.0.1:1"),
        vec![
            NarrativeSubject::new("ent-northwind", "NWSC"),
            NarrativeSubject::new("ent-orion", "NWSC"),
        ],
        series(),
    )
    .expect_err("an ambiguous issuer key was accepted");

    assert_eq!(error.code(), "invalid", "got {error:?}");
    assert!(error.message().contains("NWSC"), "{error}");
}

#[test]
fn a_series_id_that_would_split_the_request_line_is_refused_when_it_is_configured() {
    let error = NarrativeAdapter::new(
        config("http://127.0.0.1:1"),
        subjects(),
        vec!["US CPI&admin=1".to_string()],
    )
    .expect_err("a series id carrying a space and an ampersand was accepted");

    assert_eq!(error.code(), "invalid", "got {error:?}");
}

#[test]
fn a_credential_header_the_transport_writes_itself_is_refused_at_configuration_time() {
    let mut settings = config("http://127.0.0.1:1");
    settings.api_key_header = "Host".into();

    let error = NarrativeAdapter::new(settings, subjects(), series())
        .expect_err("a credential in a client-owned header was accepted");

    assert_eq!(error.code(), "invalid", "got {error:?}");
    assert!(
        error.message().contains("without a credential"),
        "the refusal must say what would have happened: {error}"
    );
}

// --- beside the synthetic sources -------------------------------------------

#[test]
fn documents_from_a_live_fetch_publish_through_the_ingestion_service() {
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let now = poll_instant();
    let (context, _clock) = Context::deterministic(now, 1);
    let log = Rc::new(RefCell::new(EventLog::in_memory()));
    let mut bus = EventBus::new().with_log(log.clone());
    let mut service = IngestionService::new(Telemetry::silent());

    service.register(Box::new(adapter_for(&server)));
    service.start(now).expect("a configured adapter starts");
    let published = service
        .poll_and_publish(&context, &mut bus, now)
        .expect("the poll succeeds");
    bus.drain(&context).expect("the bus drains");

    assert_eq!(
        published, 4,
        "every decoded record passed validation and published"
    );
    let log = log.borrow();
    assert_eq!(log.by_topic(Topic::NewsReceived).len(), 1);
    assert_eq!(log.by_topic(Topic::FundamentalUpdated).len(), 2);
    assert_eq!(log.by_topic(Topic::MacroUpdated).len(), 1);
    assert!(
        service.non_production_sources().is_empty(),
        "a licensed document feed is not a stand-in"
    );
}

#[test]
fn the_fetch_helper_returns_what_the_vendor_sent_without_the_knowable_gate() {
    let server = TestServer::always(Action::json(200, FULL_PAYLOAD));
    let mut settings = config(&server.url());
    settings.publication_delay = Duration::from_hours(24);
    let mut adapter =
        NarrativeAdapter::new(settings, subjects(), series()).expect("the configuration builds");

    assert!(
        adapter
            .poll(poll_instant())
            .expect("the poll succeeds")
            .is_empty(),
        "the premise: on a one-day dissemination delay nothing here is knowable yet"
    );
    let fetched = adapter.fetch(poll_instant()).expect("the fetch succeeds");
    assert_eq!(
        fetched.len(),
        4,
        "the helper reports what the vendor actually sent"
    );
}
