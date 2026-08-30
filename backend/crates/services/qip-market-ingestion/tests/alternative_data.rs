//! The alternative-data adapter, against a real socket.
//!
//! Every test here binds a listener on loopback and lets the adapter connect to
//! it, for the reason `rest_feed.rs` gives.
//!
//! Two properties get more attention than the rest, because they are the two
//! that fail silently. A reading keyed on the instant its image was captured
//! rather than the instant its number was published is a backtest reading the
//! future, and nothing downstream can see it. A vendor's interpolated value
//! arriving indistinguishable from a measured one is a model fitted on data
//! that was never observed, and nothing downstream can see that either.

mod server;

use qip_core::error::Error;
use qip_core::{Context, Duration, Timestamp};
use qip_events::{EventBus, EventLog, Topic};
use qip_financial::intelligence::AlternativeDataPoint;
use qip_financial::quality::{DECISION_QUALITY_FLOOR, LicensingClass};
use qip_market_ingestion::adapter::{AlternativeDataAdapter, DataAdapter, SensedRecord};
use qip_market_ingestion::alternative::{
    AlternativeFeedAdapter, AlternativeFeedConfig, AlternativeSubject,
};
use qip_market_ingestion::{IngestionService, SourceDescriptor};
use qip_observability::Telemetry;
use qip_transport::ClientLimits;
use server::{Action, TestServer, address_with_no_listener};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration as StdDuration;

/// The credential the fixtures use. A literal so a test can assert it never
/// reaches a URL or an error message.
const API_KEY: &str = "alt-key-4d19";

/// How close two `f64`s have to be to count as the same number here. The
/// quality arithmetic is a couple of multiplications, so anything wider than
/// this would stop the assertions meaning what they say.
const EPSILON: f64 = 1e-12;

fn at(text: &str) -> Timestamp {
    Timestamp::parse_rfc3339(text).expect("a fixture timestamp is valid RFC 3339")
}

/// Well after the fixture readings were published, so the knowable gate is out
/// of the way except where a test puts it back.
fn poll_instant() -> Timestamp {
    at("2026-08-24T15:00:00Z")
}

fn datasets() -> Vec<String> {
    vec![
        "satellite.parking_lot_counts".into(),
        "mobility.footfall".into(),
    ]
}

fn subjects() -> Vec<AlternativeSubject> {
    vec![AlternativeSubject::new("ent-northwind", "NWSC-US")]
}

/// Short enough that a test asserting a timeout does not sit on it.
fn tight() -> ClientLimits {
    ClientLimits {
        max_body: 64 * 1024,
        max_headers: 32,
        connect_timeout: StdDuration::from_millis(500),
        read_timeout: StdDuration::from_millis(500),
        write_timeout: StdDuration::from_millis(500),
        ..ClientLimits::default()
    }
}

fn config(base: &str) -> AlternativeFeedConfig {
    AlternativeFeedConfig {
        name: "test-alt".into(),
        provider: "a loopback alternative-data vendor".into(),
        base_url: Some(base.to_string()),
        api_key: Some(API_KEY.into()),
        // Deliberately none: most tests here are about what happens when a
        // reading states its own class or states nothing at all, and a feed-wide
        // default would mask both.
        licensing: None,
        publication_delay: Duration::ZERO,
        http: tight(),
        ..AlternativeFeedConfig::default()
    }
}

fn adapter_for(server: &TestServer) -> AlternativeFeedAdapter {
    AlternativeFeedAdapter::new(config(&server.url()), datasets(), subjects())
        .expect("the fixture configuration is valid")
}

fn adapter_serving(body: &str) -> (TestServer, AlternativeFeedAdapter) {
    let server = TestServer::always(Action::json(200, body));
    let adapter = adapter_for(&server);
    (server, adapter)
}

fn point_of(records: &[SensedRecord]) -> &AlternativeDataPoint {
    records
        .iter()
        .find_map(|record| match record {
            SensedRecord::AlternativeData(point) => Some(point.as_ref()),
            _ => None,
        })
        .expect("the records contain an alternative-data point")
}

/// A satellite count: captured on the 3rd, processed on the 9th, published on
/// the 12th. Nine days between the image and the number anyone could act on.
const READING: &str = r#"{"readings": [{
  "observation_id": "obs-8801",
  "dataset": "satellite.parking_lot_counts",
  "subject": "NWSC-US",
  "metric": "vehicles",
  "value": 1842.0,
  "unit": "vehicles",
  "captured_at": "2026-08-03T10:14:00Z",
  "processed_at": "2026-08-09T02:30:00Z",
  "published_at": "2026-08-12T06:00:00Z",
  "lead_days": 21.0,
  "proxy_correlation": 0.62,
  "proxies_for": "revenue",
  "licensing": "restricted",
  "quality": {"completeness": 0.94, "confidence": 0.88, "basis": "observed"}
}]}"#;

// --- the reading ------------------------------------------------------------

#[test]
fn a_configured_adapter_fetches_over_a_real_socket_and_decodes_a_reading() {
    let (server, mut adapter) = adapter_serving(READING);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    assert_eq!(server.served(), 1, "the adapter actually opened a socket");

    let point = point_of(&records);
    assert_eq!(point.dataset, "satellite.parking_lot_counts");
    assert_eq!(
        point.subject_id, "ent-northwind",
        "the vendor's key should be resolved to this platform's id, not passed through"
    );
    assert_eq!(point.metric, "vehicles");
    assert!((point.value - 1842.0).abs() < EPSILON);
    assert_eq!(point.unit, "vehicles");
    assert_eq!(point.proxies_for.as_deref(), Some("revenue"));
    assert!((point.lead_days - 21.0).abs() < EPSILON);
    assert!((point.proxy_correlation - 0.62).abs() < EPSILON);
    assert_eq!(
        point.provenance.upstream_id.as_deref(),
        Some("obs-8801"),
        "the vendor's own identity is kept so a reconciliation has something to join on"
    );
    assert_eq!(point.provenance.source, "test-alt");
    assert_eq!(adapter.stats().fetches, 1);
    assert_eq!(adapter.stats().emitted, 1);
}

// --- the three instants -----------------------------------------------------

#[test]
fn a_reading_is_stamped_with_the_instant_it_was_captured_and_gated_on_the_one_it_was_published() {
    let (_server, mut adapter) = adapter_serving(READING);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    let point = point_of(&records);

    assert_eq!(
        point.observed_at,
        at("2026-08-03T10:14:00Z"),
        "`observed_at` means when the phenomenon was observed, so it is the capture instant — \
         the reading's valid time"
    );
    assert_eq!(
        point.provenance.event_time,
        at("2026-08-12T06:00:00Z"),
        "the publication instant is what a consumer could have acted at, and it is recorded so \
         an audit can subtract the two and see the nine-day lag"
    );
    assert_ne!(
        point.observed_at, point.provenance.event_time,
        "the whole point of keeping both is that they differ"
    );
    assert_eq!(
        point.provenance.ingestion_time,
        poll_instant(),
        "the caller's clock, not the wall clock: the same fetch replayed in a backtest has to \
         produce the same record"
    );
}

#[test]
fn a_reading_captured_long_ago_but_published_after_the_callers_clock_is_withheld() {
    // Captured in July, published half an hour after the poll instant. Keyed on
    // capture it would look knowable for weeks; keyed on publication it is not
    // knowable yet.
    const NOT_YET: &str = r#"{"readings": [{
      "observation_id": "obs-9002", "dataset": "mobility.footfall", "subject": "NWSC-US",
      "metric": "visits", "value": 51000.0, "unit": "visits",
      "captured_at": "2026-07-01T09:00:00Z",
      "processed_at": "2026-07-04T09:00:00Z",
      "published_at": "2026-08-24T15:30:00Z",
      "licensing": "licensed",
      "quality": {"completeness": 1.0, "confidence": 0.9, "basis": "observed"}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(NOT_YET);

    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    assert!(
        records.is_empty(),
        "a reading published after the caller's clock has not been released to this deployment \
         yet, however old the image behind it is, got {records:?}"
    );
    assert_eq!(adapter.stats().withheld, 1);

    let later = adapter
        .poll(at("2026-08-24T16:00:00Z"))
        .expect("the later poll succeeds");
    assert_eq!(
        point_of(&later).observed_at,
        at("2026-07-01T09:00:00Z"),
        "and once the clock passes the publication instant, the same reading arrives still \
         carrying the July capture as its valid time"
    );
}

#[test]
fn the_publication_delay_pushes_the_knowable_instant_out_rather_than_the_capture_instant() {
    let server = TestServer::always(Action::json(200, READING));
    let mut adapter = AlternativeFeedAdapter::new(
        AlternativeFeedConfig {
            publication_delay: Duration::from_days(3),
            ..config(&server.url())
        },
        datasets(),
        subjects(),
    )
    .expect("the fixture configuration is valid");

    // Published on the 12th, so with a three-day entitlement delay it is
    // knowable on the 15th and not on the 13th.
    let early = adapter
        .poll(at("2026-08-13T00:00:00Z"))
        .expect("the poll succeeds");
    assert!(early.is_empty(), "got {early:?}");
    assert_eq!(adapter.stats().withheld, 1);

    let ready = adapter
        .poll(at("2026-08-16T00:00:00Z"))
        .expect("the poll succeeds");
    assert_eq!(point_of(&ready).observed_at, at("2026-08-03T10:14:00Z"));
}

#[test]
fn a_reading_processed_before_it_was_captured_is_refused_rather_than_reordered() {
    const IMPOSSIBLE: &str = r#"{"readings": [{
      "observation_id": "obs-9100", "dataset": "mobility.footfall", "subject": "NWSC-US",
      "metric": "visits", "value": 10.0, "unit": "visits",
      "captured_at": "2026-08-09T00:00:00Z",
      "processed_at": "2026-08-03T00:00:00Z",
      "published_at": "2026-08-12T00:00:00Z",
      "licensing": "licensed",
      "quality": {"completeness": 1.0, "confidence": 1.0, "basis": "observed"}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(IMPOSSIBLE);
    let text = adapter
        .poll(poll_instant())
        .expect_err("a number derived from an observation that had not happened is refused")
        .to_string();
    assert!(text.contains("obs-9100"), "{text}");
    assert!(
        text.contains("had not happened yet"),
        "the refusal should say what is impossible about it: {text}"
    );
}

#[test]
fn a_reading_published_before_it_was_processed_is_refused_because_its_anchor_cannot_be_trusted() {
    const IMPOSSIBLE: &str = r#"{"readings": [{
      "observation_id": "obs-9101", "dataset": "mobility.footfall", "subject": "NWSC-US",
      "metric": "visits", "value": 10.0, "unit": "visits",
      "captured_at": "2026-08-01T00:00:00Z",
      "processed_at": "2026-08-09T00:00:00Z",
      "published_at": "2026-08-04T00:00:00Z",
      "licensing": "licensed",
      "quality": {"completeness": 1.0, "confidence": 1.0, "basis": "observed"}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(IMPOSSIBLE);
    let text = adapter
        .poll(poll_instant())
        .expect_err("a reading released before the number existed is refused")
        .to_string();
    assert!(
        text.contains("released before the number existed"),
        "{text}"
    );
    assert!(
        text.contains("leakage"),
        "and why taking the earlier instant would be wrong: {text}"
    );
}

#[test]
fn a_reading_that_omits_one_of_the_three_instants_is_refused_rather_than_given_a_default() {
    const NO_PROCESSED: &str = r#"{"readings": [{
      "observation_id": "obs-9102", "dataset": "mobility.footfall", "subject": "NWSC-US",
      "metric": "visits", "value": 10.0, "unit": "visits",
      "captured_at": "2026-08-01T00:00:00Z",
      "published_at": "2026-08-04T00:00:00Z",
      "licensing": "licensed",
      "quality": {"completeness": 1.0, "confidence": 1.0, "basis": "observed"}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(NO_PROCESSED);
    let text = adapter
        .poll(poll_instant())
        .expect_err("a defaulted instant is a claim about when a number could be acted on")
        .to_string();
    assert!(text.contains("processed_at"), "{text}");
}

#[test]
fn readings_come_back_in_capture_order_whatever_order_the_vendor_listed_them_in() {
    const OUT_OF_ORDER: &str = r#"{"readings": [
      {"observation_id": "obs-b", "dataset": "mobility.footfall", "subject": "NWSC-US",
       "metric": "visits", "value": 2.0, "unit": "visits",
       "captured_at": "2026-08-05T00:00:00Z", "processed_at": "2026-08-06T00:00:00Z",
       "published_at": "2026-08-07T00:00:00Z", "licensing": "licensed",
       "quality": {"completeness": 1.0, "confidence": 1.0, "basis": "observed"}},
      {"observation_id": "obs-a", "dataset": "mobility.footfall", "subject": "NWSC-US",
       "metric": "visits", "value": 1.0, "unit": "visits",
       "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-06T00:00:00Z",
       "published_at": "2026-08-07T00:00:00Z", "licensing": "licensed",
       "quality": {"completeness": 1.0, "confidence": 1.0, "basis": "observed"}}
    ]}"#;
    let (_server, mut adapter) = adapter_serving(OUT_OF_ORDER);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    assert_eq!(records.len(), 2);
    assert!(
        records[0].occurred_at() <= records[1].occurred_at(),
        "a consumer assuming a monotone stream must not be reading the vendor's array order"
    );
}

// --- licensing --------------------------------------------------------------

#[test]
fn a_reading_with_no_licensing_class_and_no_feed_default_is_refused_rather_than_defaulted() {
    const UNCLASSED: &str = r#"{"readings": [{
      "observation_id": "obs-7001", "dataset": "mobility.footfall", "subject": "NWSC-US",
      "metric": "visits", "value": 42.0, "unit": "visits",
      "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
      "published_at": "2026-08-03T00:00:00Z",
      "quality": {"completeness": 1.0, "confidence": 1.0, "basis": "observed"}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(UNCLASSED);

    let error = adapter
        .poll(poll_instant())
        .expect_err("an unstated licence is not a permissive one");
    assert!(
        matches!(error, Error::Denied { .. }),
        "refusing on licensing grounds is a denial, not a schema complaint: {error:?}"
    );
    let text = error.to_string();
    assert!(text.contains("obs-7001"), "{text}");
    assert!(
        text.contains("Internal"),
        "the refusal should name the default it is refusing to fall back to: {text}"
    );
    assert!(
        text.contains("permits raw display"),
        "and say why that default is not a safe one — an unset class reads downstream as \
         permission, not as unknown: {text}"
    );
    assert_eq!(adapter.stats().emitted, 0);
}

#[test]
fn the_default_licensing_class_really_would_have_permitted_raw_display() {
    // The premise the refusal above rests on. If this ever stopped being true,
    // that test would be asserting a rule that no longer protects anything.
    assert_eq!(LicensingClass::default(), LicensingClass::Internal);
    assert!(
        LicensingClass::default().allows_raw_display(),
        "an unset class is not a neutral one: it is a grant, which is exactly why the adapter \
         refuses rather than falling back to it"
    );
    assert!(!LicensingClass::Restricted.allows_raw_display());
}

#[test]
fn every_decoded_reading_carries_the_licensing_class_its_vendor_stated() {
    let (_server, mut adapter) = adapter_serving(READING);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    let point = point_of(&records);
    assert_eq!(point.provenance.licensing, LicensingClass::Restricted);
    assert!(
        !point.provenance.licensing.allows_raw_display(),
        "the class travels with the record so the boundary that would display it can decide"
    );
    assert!(
        !point.value.is_nan(),
        "labelling is not redaction: a restricted reading still arrives with its value, because \
         a model's use of it is a derived use"
    );
}

#[test]
fn a_feed_wide_licensing_class_covers_a_reading_that_states_none() {
    const UNCLASSED: &str = r#"{"readings": [{
      "observation_id": "obs-7002", "dataset": "mobility.footfall", "subject": "NWSC-US",
      "metric": "visits", "value": 42.0, "unit": "visits",
      "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
      "published_at": "2026-08-03T00:00:00Z",
      "quality": {"completeness": 1.0, "confidence": 1.0, "basis": "observed"}
    }]}"#;
    let server = TestServer::always(Action::json(200, UNCLASSED));
    let mut adapter = AlternativeFeedAdapter::new(
        AlternativeFeedConfig {
            licensing: Some(LicensingClass::Licensed),
            ..config(&server.url())
        },
        datasets(),
        subjects(),
    )
    .expect("the fixture configuration is valid");

    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    assert_eq!(
        point_of(&records).provenance.licensing,
        LicensingClass::Licensed,
        "a deployment that has agreed one set of terms for the whole feed may say so; what it \
         may not do is leave the question unanswered"
    );
}

#[test]
fn a_licensing_class_this_decoder_cannot_name_is_refused_rather_than_mapped_to_the_nearest() {
    const ODD: &str = r#"{"readings": [{
      "observation_id": "obs-7003", "dataset": "mobility.footfall", "subject": "NWSC-US",
      "metric": "visits", "value": 42.0, "unit": "visits",
      "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
      "published_at": "2026-08-03T00:00:00Z", "licensing": "evaluation_only",
      "quality": {"completeness": 1.0, "confidence": 1.0, "basis": "observed"}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(ODD);
    let text = adapter
        .poll(poll_instant())
        .expect_err("an unknown class is refused")
        .to_string();
    assert!(text.contains("evaluation_only"), "{text}");
    assert!(
        text.contains("public") && text.contains("restricted"),
        "the refusal should list what this decoder does accept: {text}"
    );
}

#[test]
fn an_unconfigured_feed_describes_itself_with_the_class_that_forbids_raw_display() {
    let adapter =
        AlternativeFeedAdapter::new(AlternativeFeedConfig::default(), datasets(), subjects())
            .expect("an adapter with no licence configured still has to exist");
    let descriptor: SourceDescriptor = adapter.descriptor();
    assert_eq!(
        descriptor.licensing,
        LicensingClass::Restricted,
        "reporting the `Internal` default would tell a caller that raw values may be displayed, \
         which is the one thing an unconfigured feed cannot promise"
    );
    assert!(
        descriptor.is_production_grade(),
        "restricted is strict about display and says nothing against a capital decision"
    );
}

#[test]
fn a_vendor_that_refuses_on_legal_grounds_says_so_rather_than_looking_like_an_outage() {
    let server = TestServer::always(Action::json(451, r#"{"error":"not licensed"}"#));
    let mut adapter = adapter_for(&server);
    let error = adapter
        .poll(poll_instant())
        .expect_err("a 451 is a refusal");
    assert!(matches!(error, Error::Denied { .. }), "{error:?}");
    let text = error.to_string();
    assert!(
        text.contains("Widening the request is not the fix"),
        "an operator reading this needs to know the agreement is the problem: {text}"
    );
}

// --- quality and imputation -------------------------------------------------

#[test]
fn a_value_the_vendor_filled_in_arrives_marked_imputed_and_naming_how() {
    const IMPUTED: &str = r#"{"readings": [{
      "observation_id": "obs-6001", "dataset": "satellite.parking_lot_counts",
      "subject": "NWSC-US", "metric": "vehicles", "value": 1500.0, "unit": "vehicles",
      "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
      "published_at": "2026-08-03T00:00:00Z", "licensing": "licensed",
      "quality": {"completeness": 0.5, "confidence": 0.9,
                  "basis": "imputed",
                  "method": "last observation carried forward across an 11-day cloud gap"}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(IMPUTED);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    let point = point_of(&records);

    assert!(
        point.quality.is_imputed,
        "a value the vendor filled in must never arrive as observed fact"
    );
    assert!(
        point
            .quality
            .issues
            .iter()
            .any(|issue| issue.contains("11-day cloud gap")),
        "and the method travels with it, because \"we filled this gap\" and \"we carried the \
         last value forward for eleven days\" are different facts: {:?}",
        point.quality.issues
    );
    assert_eq!(point.quality.validation_failures, 1);
    // 0.9, halved by the recorded issue, then four fifths of that for the
    // imputation — the platform's own arithmetic, applied to a vendor's
    // reading exactly as it is applied to everything else.
    assert!(
        (point.quality.confidence - 0.9 * 0.5 * 0.8).abs() < EPSILON,
        "confidence should fall for the imputation rather than staying at what the vendor \
         claimed, got {}",
        point.quality.confidence
    );
    assert!(
        !point.quality.meets(DECISION_QUALITY_FLOOR),
        "half-complete and interpolated should not clear the floor a capital decision needs"
    );
    assert!(
        !point.is_actionable(),
        "and the record itself should say so rather than leaving a caller to work it out"
    );
    assert_eq!(adapter.stats().imputed, 1);
}

#[test]
fn an_observed_value_is_not_marked_imputed_and_keeps_the_confidence_the_vendor_stated() {
    let (_server, mut adapter) = adapter_serving(READING);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    let point = point_of(&records);

    assert!(!point.quality.is_imputed);
    assert_eq!(point.quality.validation_failures, 0);
    assert!(point.quality.issues.is_empty());
    assert!((point.quality.completeness - 0.94).abs() < EPSILON);
    assert!(
        (point.quality.confidence - 0.88).abs() < EPSILON,
        "a reading the vendor measured and declared no failures on keeps what it stated"
    );
    assert!(point.quality.meets(DECISION_QUALITY_FLOOR));
    assert!(
        point.is_actionable(),
        "a well-measured proxy with a correlation of 0.62 naming what it proxies for is one the \
         platform may act on"
    );
    assert_eq!(adapter.stats().imputed, 0);
}

#[test]
fn a_reading_that_states_no_quality_at_all_is_refused_rather_than_given_the_clean_default() {
    const NO_QUALITY: &str = r#"{"readings": [{
      "observation_id": "obs-6002", "dataset": "mobility.footfall", "subject": "NWSC-US",
      "metric": "visits", "value": 42.0, "unit": "visits",
      "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
      "published_at": "2026-08-03T00:00:00Z", "licensing": "licensed"
    }]}"#;
    let (_server, mut adapter) = adapter_serving(NO_QUALITY);
    let text = adapter
        .poll(poll_instant())
        .expect_err("an unstated quality is not a clean one")
        .to_string();
    assert!(text.contains("obs-6002"), "{text}");
    assert!(
        text.contains("completeness 1.0"),
        "the refusal should name what the default would have asserted: {text}"
    );
    assert!(
        text.contains("not imputed"),
        "including that it would have claimed the value was measured: {text}"
    );
}

#[test]
fn the_default_quality_really_would_have_asserted_a_perfectly_measured_value() {
    // The premise the refusal above rests on.
    let default = qip_financial::quality::DataQuality::default();
    assert!(!default.is_imputed);
    assert!((default.completeness - 1.0).abs() < EPSILON);
    assert!((default.confidence - 1.0).abs() < EPSILON);
    assert!(
        default.meets(DECISION_QUALITY_FLOOR),
        "so a vendor that said nothing would produce a reading cleared for a capital decision, \
         which is exactly what the refusal exists to prevent"
    );
}

#[test]
fn a_value_marked_imputed_that_names_no_method_is_refused() {
    const NO_METHOD: &str = r#"{"readings": [{
      "observation_id": "obs-6003", "dataset": "mobility.footfall", "subject": "NWSC-US",
      "metric": "visits", "value": 42.0, "unit": "visits",
      "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
      "published_at": "2026-08-03T00:00:00Z", "licensing": "licensed",
      "quality": {"completeness": 0.5, "confidence": 0.9, "basis": "imputed", "method": "   "}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(NO_METHOD);
    let text = adapter
        .poll(poll_instant())
        .expect_err("an imputation with no method cannot be reasoned about")
        .to_string();
    assert!(text.contains("names no method"), "{text}");
}

#[test]
fn a_basis_the_vendor_left_out_entirely_is_refused_rather_than_read_as_observed() {
    const NO_BASIS: &str = r#"{"readings": [{
      "observation_id": "obs-6004", "dataset": "mobility.footfall", "subject": "NWSC-US",
      "metric": "visits", "value": 42.0, "unit": "visits",
      "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
      "published_at": "2026-08-03T00:00:00Z", "licensing": "licensed",
      "quality": {"completeness": 0.5, "confidence": 0.9}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(NO_BASIS);
    adapter
        .poll(poll_instant())
        .expect_err("there is no third basis for \"the vendor did not say\"");
}

#[test]
fn the_checks_a_vendor_declares_it_failed_become_validation_failures_and_lower_confidence() {
    const WITH_ISSUES: &str = r#"{"readings": [{
      "observation_id": "obs-6005", "dataset": "satellite.parking_lot_counts",
      "subject": "NWSC-US", "metric": "vehicles", "value": 900.0, "unit": "vehicles",
      "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
      "published_at": "2026-08-03T00:00:00Z", "licensing": "licensed",
      "quality": {"completeness": 0.8, "confidence": 0.96, "basis": "observed",
                  "issues": ["cloud cover 38%", "off-nadir angle beyond tolerance"]}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(WITH_ISSUES);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    let point = point_of(&records);

    assert_eq!(point.quality.validation_failures, 2);
    assert_eq!(point.quality.issues.len(), 2);
    assert!(
        (point.quality.confidence - 0.96 * 0.5 * 0.5).abs() < EPSILON,
        "a vendor that declares both a high confidence and the checks that failed has told us \
         two things, and the lower of them is what should reach a decision, got {}",
        point.quality.confidence
    );
    assert!(!point.quality.is_imputed);
    assert_eq!(adapter.stats().with_declared_failures, 1);
}

#[test]
fn a_confidence_outside_its_range_is_refused_here_because_nothing_downstream_rechecks_it() {
    const OUT_OF_RANGE: &str = r#"{"readings": [{
      "observation_id": "obs-6006", "dataset": "mobility.footfall", "subject": "NWSC-US",
      "metric": "visits", "value": 42.0, "unit": "visits",
      "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
      "published_at": "2026-08-03T00:00:00Z", "licensing": "licensed",
      "quality": {"completeness": 1.0, "confidence": 4.2, "basis": "observed"}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(OUT_OF_RANGE);
    let text = adapter
        .poll(poll_instant())
        .expect_err("a confidence of 4.2 does not mean what this decoder reads it as")
        .to_string();
    assert!(text.contains("confidence of 4.2"), "{text}");
    assert!(
        text.contains("clamp"),
        "and it should say why this one is refused here rather than passed to the validation \
         gate: nothing downstream re-checks it, it would simply be clamped: {text}"
    );
}

#[test]
fn an_implausible_but_finite_value_is_passed_through_rather_than_corrected_here() {
    // The house rule `rest.rs` set: an adapter that quietly corrected bad
    // vendor data would make it invisible. A footfall count cannot be negative,
    // and this one reaches the bus as the vendor sent it.
    const NEGATIVE: &str = r#"{"readings": [{
      "observation_id": "obs-6007", "dataset": "mobility.footfall", "subject": "NWSC-US",
      "metric": "visits", "value": -5000.0, "unit": "visits",
      "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
      "published_at": "2026-08-03T00:00:00Z", "licensing": "licensed",
      "quality": {"completeness": 1.0, "confidence": 1.0, "basis": "observed"}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(NEGATIVE);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    assert!(
        (point_of(&records).value + 5000.0).abs() < EPSILON,
        "the value should arrive exactly as the vendor sent it, neither clamped nor dropped"
    );
}

#[test]
fn the_validation_gate_cannot_catch_a_bad_reading_which_is_why_this_module_refuses_instead() {
    // The premise behind refusing an out-of-range confidence and an unstated
    // quality here rather than passing them on. `rest.rs` can hand an incoherent
    // bar downstream because `SensedRecord::validate` checks bars; for an
    // alternative-data point it checks one thing, and JSON cannot even express
    // the value that would fail it.
    let (_server, mut adapter) = adapter_serving(READING);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    assert!(
        records[0].validate().is_empty(),
        "a well-formed reading passes, as it should"
    );

    let refused = serde_json::from_str::<serde_json::Value>(r#"{"value": 1e400}"#);
    assert!(
        refused.is_err(),
        "and a non-finite value — the only thing the gate rejects for this record type — cannot \
         arrive over JSON at all. So there is no downstream check to defer to, and the quality \
         and ordering rules in this module have to be refusals rather than pass-throughs"
    );
}

// --- coverage ---------------------------------------------------------------

#[test]
fn a_dataset_this_deployment_does_not_cover_is_refused() {
    const UNCOVERED: &str = r#"{"readings": [{
      "observation_id": "obs-5001", "dataset": "web.job_postings", "subject": "NWSC-US",
      "metric": "openings", "value": 42.0, "unit": "postings",
      "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
      "published_at": "2026-08-03T00:00:00Z", "licensing": "licensed",
      "quality": {"completeness": 1.0, "confidence": 1.0, "basis": "observed"}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(UNCOVERED);
    let text = adapter
        .poll(poll_instant())
        .expect_err("a dataset nobody configured is one nobody modelled")
        .to_string();
    assert!(text.contains("web.job_postings"), "{text}");
    assert!(text.contains("satellite.parking_lot_counts"), "{text}");
}

#[test]
fn a_subject_this_deployment_has_not_mapped_is_refused_not_given_an_invented_id() {
    const UNMAPPED: &str = r#"{"readings": [{
      "observation_id": "obs-5002", "dataset": "mobility.footfall", "subject": "VNTG-US",
      "metric": "visits", "value": 42.0, "unit": "visits",
      "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
      "published_at": "2026-08-03T00:00:00Z", "licensing": "licensed",
      "quality": {"completeness": 1.0, "confidence": 1.0, "basis": "observed"}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(UNMAPPED);
    let text = adapter
        .poll(poll_instant())
        .expect_err("a reading whose subject cannot be identified is refused")
        .to_string();
    assert!(text.contains("VNTG-US"), "{text}");
    assert!(
        text.contains("merge it with another subject"),
        "the refusal should say what an invented id would cost: {text}"
    );
}

#[test]
fn a_reading_with_no_observation_id_or_no_metric_is_refused() {
    for (label, body) in [
        (
            "no observation id",
            r#"{"readings": [{
              "observation_id": "", "dataset": "mobility.footfall", "subject": "NWSC-US",
              "metric": "visits", "value": 1.0, "unit": "visits",
              "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
              "published_at": "2026-08-03T00:00:00Z", "licensing": "licensed",
              "quality": {"completeness": 1.0, "confidence": 1.0, "basis": "observed"}
            }]}"#,
        ),
        (
            "no metric",
            r#"{"readings": [{
              "observation_id": "obs-5003", "dataset": "mobility.footfall", "subject": "NWSC-US",
              "metric": "  ", "value": 1.0, "unit": "visits",
              "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
              "published_at": "2026-08-03T00:00:00Z", "licensing": "licensed",
              "quality": {"completeness": 1.0, "confidence": 1.0, "basis": "observed"}
            }]}"#,
        ),
    ] {
        let (_server, mut adapter) = adapter_serving(body);
        assert!(
            adapter.poll(poll_instant()).is_err(),
            "a reading with {label} should be refused"
        );
    }
}

#[test]
fn a_correlation_the_vendor_did_not_state_reads_as_not_actionable_rather_than_as_a_strong_proxy() {
    const BARE: &str = r#"{"readings": [{
      "observation_id": "obs-5004", "dataset": "mobility.footfall", "subject": "NWSC-US",
      "metric": "visits", "value": 42.0, "unit": "visits",
      "captured_at": "2026-08-01T00:00:00Z", "processed_at": "2026-08-02T00:00:00Z",
      "published_at": "2026-08-03T00:00:00Z", "licensing": "licensed",
      "quality": {"completeness": 1.0, "confidence": 1.0, "basis": "observed"}
    }]}"#;
    let (_server, mut adapter) = adapter_serving(BARE);
    let records = adapter.poll(poll_instant()).expect("the poll succeeds");
    let point = point_of(&records);
    assert!(point.proxy_correlation.abs() < EPSILON);
    assert_eq!(point.proxies_for, None);
    assert!(
        !point.is_actionable(),
        "defaulting is safe here for the one reason it is unsafe for licensing: the default is \
         the restrictive value, not the permissive one"
    );
}

// --- the peer is untrusted --------------------------------------------------

#[test]
fn a_body_larger_than_the_limit_is_refused_before_it_is_buffered() {
    let server = TestServer::always(Action::Oversized { bytes: 256 * 1024 });
    let mut adapter = adapter_for(&server);
    let text = adapter
        .poll(poll_instant())
        .expect_err("a body over the cap is refused")
        .to_string();
    assert!(
        text.contains("body") || text.contains("large") || text.contains("limit"),
        "the refusal should say the body was too large rather than that the JSON was bad: {text}"
    );
    assert_eq!(adapter.stats().emitted, 0);
}

#[test]
fn a_peer_that_dies_part_way_through_its_own_body_is_a_close_and_not_a_short_reading() {
    let server = TestServer::always(Action::Truncated {
        declared: 4096,
        written: 40,
    });
    let mut adapter = adapter_for(&server);
    adapter
        .poll(poll_instant())
        .expect_err("a half-sent body is refused rather than decoded as far as it got");
    assert_eq!(adapter.stats().fetches, 0);
    assert_eq!(adapter.stats().emitted, 0);
}

#[test]
fn a_peer_that_accepts_the_connection_and_says_nothing_is_refused_within_the_timeout() {
    let server = TestServer::always(Action::Silent(StdDuration::from_secs(30)));
    let mut adapter = adapter_for(&server);
    let started = std::time::Instant::now();
    adapter
        .poll(poll_instant())
        .expect_err("a peer that never answers is refused");
    assert!(
        started.elapsed() < StdDuration::from_secs(5),
        "the read timeout has to bound the wait; a poll loop with no bound stops polling"
    );
}

#[test]
fn an_unreachable_vendor_is_refused_rather_than_waited_on() {
    let mut adapter =
        AlternativeFeedAdapter::new(config(&address_with_no_listener()), datasets(), subjects())
            .expect("the fixture configuration is valid");
    let started = std::time::Instant::now();
    adapter
        .poll(poll_instant())
        .expect_err("nothing is listening");
    assert!(started.elapsed() < StdDuration::from_secs(5));
}

#[test]
fn a_body_that_is_not_json_is_refused_with_an_error_that_names_the_feed() {
    let (_server, mut adapter) = adapter_serving("<html>not json at all");
    let text = adapter
        .poll(poll_instant())
        .expect_err("a non-JSON body is refused")
        .to_string();
    assert!(text.contains("test-alt"), "{text}");
}

#[test]
fn more_readings_than_the_cap_are_refused_even_when_the_body_fits() {
    let readings: Vec<String> = (0..40)
        .map(|i| {
            format!(
                r#"{{"observation_id":"obs-{i}","dataset":"mobility.footfall",
                     "subject":"NWSC-US","metric":"visits","value":1.0,"unit":"visits",
                     "captured_at":"2026-08-01T00:00:00Z",
                     "processed_at":"2026-08-02T00:00:00Z",
                     "published_at":"2026-08-03T00:00:00Z","licensing":"licensed",
                     "quality":{{"completeness":1.0,"confidence":1.0,"basis":"observed"}}}}"#
            )
        })
        .collect();
    let body = format!(r#"{{"readings":[{}]}}"#, readings.join(","));
    let server = TestServer::always(Action::json(200, body));
    let mut adapter = AlternativeFeedAdapter::new(
        AlternativeFeedConfig {
            max_records: 10,
            ..config(&server.url())
        },
        datasets(),
        subjects(),
    )
    .expect("the fixture configuration is valid");
    let text = adapter
        .poll(poll_instant())
        .expect_err("40 readings against a cap of 10 is refused")
        .to_string();
    assert!(text.contains("40 readings"), "{text}");
}

#[test]
fn a_vendor_that_rejects_the_credential_produces_a_denial_that_does_not_quote_it() {
    let server = TestServer::always(Action::json(403, r#"{"error":"forbidden"}"#));
    let mut adapter = adapter_for(&server);
    let text = adapter
        .poll(poll_instant())
        .expect_err("a 403 is a refusal")
        .to_string();
    assert!(text.contains("403"), "{text}");
    assert!(
        !text.contains(API_KEY),
        "a credential in an error message is a credential in a log: {text}"
    );
}

// --- configuration ----------------------------------------------------------

#[test]
fn an_unconfigured_adapter_names_every_missing_piece_and_opens_no_connection() {
    let server = TestServer::always(Action::json(200, READING));
    let mut adapter =
        AlternativeFeedAdapter::new(AlternativeFeedConfig::default(), Vec::new(), Vec::new())
            .expect("an adapter with nothing configured still has to exist in order to say so");

    assert!(!adapter.is_available());
    let missing = adapter.missing_configuration();
    assert_eq!(
        missing.len(),
        4,
        "endpoint, credential, datasets and subjects should each be named on their own: \
         {missing:?}"
    );
    assert!(
        missing.iter().any(|m| m.contains("no endpoint")),
        "{missing:?}"
    );
    assert!(
        missing.iter().any(|m| m.contains("no credential")),
        "{missing:?}"
    );
    assert!(
        missing.iter().any(|m| m.contains("no datasets")),
        "{missing:?}"
    );
    assert!(
        missing.iter().any(|m| m.contains("no subjects")),
        "{missing:?}"
    );

    let start = adapter.start(poll_instant());
    assert!(matches!(start, Err(Error::Unavailable { .. })), "{start:?}");
    let polled = adapter.poll(poll_instant());
    assert!(
        matches!(polled, Err(Error::Unavailable { .. })),
        "{polled:?}"
    );
    assert!(
        adapter
            .poll(poll_instant())
            .expect_err("still unavailable")
            .to_string()
            .contains("will not substitute generated readings")
    );

    assert_eq!(
        server.served(),
        0,
        "an unconfigured adapter must not open a socket at all: there is nothing to ask and \
         nowhere to ask it"
    );
}

#[test]
fn an_endpoint_without_a_credential_is_still_unavailable_and_still_opens_nothing() {
    let server = TestServer::always(Action::json(200, READING));
    let mut adapter = AlternativeFeedAdapter::new(
        AlternativeFeedConfig {
            api_key: None,
            ..config(&server.url())
        },
        datasets(),
        subjects(),
    )
    .expect("the fixture configuration is valid");

    assert!(!adapter.is_available());
    adapter
        .poll(poll_instant())
        .expect_err("no credential means no fetch");
    assert_eq!(server.served(), 0, "and no connection either");
}

#[test]
fn a_configured_adapter_still_states_what_production_has_to_add() {
    let server = TestServer::always(Action::json(200, READING));
    let adapter = adapter_for(&server);
    assert!(adapter.is_available());

    let descriptor = adapter.descriptor();
    let requirement = descriptor
        .production_requirement
        .clone()
        .expect("a fully configured alt-data feed is still not a production feed on its own");
    assert!(requirement.contains("TLS"), "{requirement}");
    assert!(
        requirement.contains("no feed-wide licensing class is configured"),
        "this fixture configures no default, so the requirement should say every reading must \
         state its own: {requirement}"
    );
    assert!(requirement.contains("dissemination delay"), "{requirement}");
    assert!(
        requirement.contains("measured or filled in"),
        "the vendor has to declare imputation, and this adapter cannot detect it otherwise: \
         {requirement}"
    );
    assert!(
        requirement.contains("imputation rate"),
        "and the deployment has to alert on it: {requirement}"
    );
    assert_eq!(descriptor.topics, vec![Topic::AlternativeDataReceived]);
    let mut configured = datasets();
    configured.sort();
    assert_eq!(
        adapter.datasets(),
        configured,
        "coverage is reported in a stable order rather than in the order it was configured"
    );
}

#[test]
fn the_request_names_the_window_over_publication_time_and_the_coverage_it_was_configured_with() {
    let server = TestServer::always(Action::json(200, READING));
    let mut adapter = adapter_for(&server);
    adapter.poll(poll_instant()).expect("the poll succeeds");

    let requests = server.requests();
    let target = &requests[0].target;
    assert!(
        target.contains("published_since=") && target.contains("published_until="),
        "the window is over publication time, and the parameter name is the only place this \
         adapter can say so to a vendor filtering by capture time instead: {target}"
    );
    assert!(target.contains("mobility.footfall"), "{target}");
    assert!(target.contains("NWSC-US"), "{target}");
    assert!(
        target.contains("published_until=2026-08-24T15:00:00"),
        "{target}"
    );
}

#[test]
fn the_credential_travels_in_a_header_and_never_in_the_url() {
    let server = TestServer::always(Action::json(200, READING));
    let mut adapter = adapter_for(&server);
    adapter.poll(poll_instant()).expect("the poll succeeds");

    for request in server.requests() {
        assert_eq!(
            request.method, "GET",
            "fetching a reading is a read, and the window travels in the query rather than in a \
             body a caching proxy cannot see"
        );
        assert!(
            !request.target.contains(API_KEY),
            "a URL is written to every access log on the path: {}",
            request.target
        );
        assert_eq!(
            request.headers.get("x-api-key").map(String::as_str),
            Some(API_KEY)
        );
    }
}

#[test]
fn an_https_endpoint_is_refused_at_configuration_time_rather_than_downgraded() {
    let built = AlternativeFeedAdapter::new(
        AlternativeFeedConfig {
            base_url: Some("https://vendor.example".into()),
            api_key: Some(API_KEY.into()),
            ..AlternativeFeedConfig::default()
        },
        datasets(),
        subjects(),
    );
    assert!(
        built.is_err(),
        "the transport has no TLS stack, so an https endpoint would send the credential in \
         clear text if it were quietly downgraded"
    );
}

#[test]
fn two_subjects_claiming_one_vendor_key_are_refused() {
    let built = AlternativeFeedAdapter::new(
        AlternativeFeedConfig::default(),
        datasets(),
        vec![
            AlternativeSubject::new("ent-northwind", "NWSC-US"),
            AlternativeSubject::new("ent-vantage", "NWSC-US"),
        ],
    );
    let text = built
        .expect_err("a reading keyed by a shared vendor key could not be resolved")
        .to_string();
    assert!(text.contains("NWSC-US"), "{text}");
}

#[test]
fn a_dataset_name_that_would_split_the_request_line_is_refused_when_it_is_configured() {
    let built = AlternativeFeedAdapter::new(
        AlternativeFeedConfig::default(),
        vec!["mobility.footfall&subjects=ALL".into()],
        subjects(),
    );
    assert!(built.is_err());
}

#[test]
fn a_credential_header_the_transport_writes_itself_is_refused_at_configuration_time() {
    let built = AlternativeFeedAdapter::new(
        AlternativeFeedConfig {
            api_key_header: "Content-Length".into(),
            api_key: Some(API_KEY.into()),
            base_url: Some("http://vendor.example".into()),
            ..AlternativeFeedConfig::default()
        },
        datasets(),
        subjects(),
    );
    let text = built
        .expect_err("the transport writes `content-length` itself and drops a caller's copy")
        .to_string();
    assert!(text.contains("content-length"), "{text}");
}

#[test]
fn a_credential_carrying_a_newline_is_refused_before_it_can_forge_a_header() {
    let built = AlternativeFeedAdapter::new(
        AlternativeFeedConfig {
            api_key: Some("key\r\nx-admin: true".into()),
            base_url: Some("http://vendor.example".into()),
            ..AlternativeFeedConfig::default()
        },
        datasets(),
        subjects(),
    );
    assert!(built.is_err());
}

#[test]
fn the_debug_rendering_of_a_configuration_shows_that_a_credential_is_set_but_never_its_value() {
    let rendered = format!("{:?}", config("http://vendor.example"));
    assert!(
        !rendered.contains(API_KEY),
        "a config in a crash dump or a support ticket must not carry the key: {rendered}"
    );
    assert!(rendered.contains("<redacted>"), "{rendered}");
}

// --- through the ingestion service ------------------------------------------

#[test]
fn readings_from_a_live_fetch_publish_through_the_ingestion_service() {
    let server = TestServer::always(Action::json(200, READING));
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

    assert_eq!(published, 1);
    let log = log.borrow();
    assert_eq!(log.by_topic(Topic::AlternativeDataReceived).len(), 1);
    assert!(
        service.non_production_sources().is_empty(),
        "a restricted feed is strict about display and is still a real source"
    );
}

#[test]
fn the_fetch_helper_returns_what_the_vendor_sent_without_the_knowable_gate() {
    let server = TestServer::always(Action::json(200, READING));
    let mut adapter = AlternativeFeedAdapter::new(
        AlternativeFeedConfig {
            publication_delay: Duration::from_days(365),
            ..config(&server.url())
        },
        datasets(),
        subjects(),
    )
    .expect("the fixture configuration is valid");

    assert!(
        adapter
            .poll(poll_instant())
            .expect("the poll succeeds")
            .is_empty(),
        "a year-long entitlement delay withholds everything"
    );
    assert_eq!(
        adapter
            .fetch(poll_instant())
            .expect("the fetch succeeds")
            .len(),
        1,
        "fetch exists so an operator can test the connection and the credential without the \
         answer depending on where the clock is"
    );
}
